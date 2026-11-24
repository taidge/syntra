//! Opt-in SQLite allocation observations, never RSS or a complete pool census.
//! No connection handles, SQL, pool owners or worker handles leave the callback.
//! SYNTRA_SQLITE_MEMORY_DIAGNOSTICS=1 is read once at the first database creation.
//! The default does not sample SQLite or emit these diagnostic events.
use std::ffi::OsStr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use diesel::SqliteConnection;
use libsqlite3_sys as ffi;

pub(super) fn enabled_from_environment() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let value = std::env::var_os("SYNTRA_SQLITE_MEMORY_DIAGNOSTICS")
            .or_else(|| std::env::var_os("\x43ONTRIX_SQLITE_MEMORY_DIAGNOSTICS"));
        opted_in(value.as_deref())
    })
}

fn opted_in(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Value {
    pub rc: i32,
    pub bytes: Option<i64>,
}

impl Value {
    fn new(rc: i32, current: i64) -> Self {
        Self {
            rc,
            bytes: (rc == ffi::SQLITE_OK && current >= 0).then_some(current),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Sample {
    pub epoch: u64,
    pub began: Instant,
    pub ended: Instant,
    pub values: [Value; 4],
}

pub(super) fn sample(connection: &mut SqliteConnection, epoch: u64) -> Sample {
    let began = Instant::now();
    // SAFETY: the existing native worker has exclusive &mut access. The live
    // borrowed handle never escapes; only synchronous status reads occur, with
    // valid out-pointers and reset=0. No close, SQL, transaction, statement or
    // connection configuration is changed.
    let values = unsafe {
        connection.with_raw_connection(|db| {
            [
                ffi::SQLITE_DBSTATUS_CACHE_USED,
                ffi::SQLITE_DBSTATUS_CACHE_USED_SHARED,
                ffi::SQLITE_DBSTATUS_SCHEMA_USED,
                ffi::SQLITE_DBSTATUS_STMT_USED,
            ]
            .map(|op| db_value(db, op))
        })
    };
    Sample {
        epoch,
        began,
        ended: Instant::now(),
        values,
    }
}

// Caller must supply the live exclusively borrowed connection from Diesel.
unsafe fn db_value(db: *mut ffi::sqlite3, op: i32) -> Value {
    let (mut current, mut highwater) = (0_i64, 0_i64);
    // SAFETY: upheld by the caller; both output pointers are valid locals.
    let rc = unsafe { ffi::sqlite3_db_status64(db, op, &mut current, &mut highwater, 0) };
    // These four metrics' highwater is documented as zero, not a measured peak.
    Value::new(rc, current)
}

pub(super) struct Diagnostics {
    // Zero disables sampling and reports. No production mutation enables it.
    epoch: AtomicU64,
    observation_disposals: AtomicU64,
}

impl Diagnostics {
    pub fn new(enabled: bool) -> Self {
        Self {
            epoch: AtomicU64::new(u64::from(enabled)),
            observation_disposals: AtomicU64::new(0),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    pub fn advance(&self) -> u64 {
        // Never wrap into disabled or reuse an old epoch.
        match self
            .epoch
            .try_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                if epoch == 0 {
                    None
                } else {
                    epoch.checked_add(1)
                }
            }) {
            Ok(epoch) => epoch + 1,
            Err(epoch) => epoch,
        }
    }

    pub fn observation_disposed(&self) {
        if self.epoch() != 0 {
            self.observation_disposals.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn disposal_events(&self) -> u64 {
        self.observation_disposals.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn set_enabled_for_test(&self, enabled: bool) {
        self.epoch.store(u64::from(enabled), Ordering::Release);
    }
}

pub(super) struct Row {
    pub sequence: u64,
    pub disposed: bool,
    pub sample: Option<Sample>,
}

pub(super) struct Report {
    pub epoch: u64,
    pub next_epoch: u64,
    pub capacity: usize,
    pub registrations: u64,
    pub disposal_events: u64,
    pub omitted_observations: u64,
    pub rows: Vec<Row>,
    global_began: Instant,
    global_ended: Instant,
    global: Value,
    global_highwater: Option<i64>,
}

impl Report {
    pub fn new(epoch: u64, next_epoch: u64, capacity: usize) -> Self {
        let (mut current, mut highwater) = (0_i64, 0_i64);
        let global_began = Instant::now();
        // SAFETY: this global read uses valid local out-pointers and reset=0;
        // it does not open a DB or change SQLite's memory configuration.
        let rc = unsafe {
            ffi::sqlite3_status64(
                ffi::SQLITE_STATUS_MEMORY_USED,
                &mut current,
                &mut highwater,
                0,
            )
        };
        let global_ended = Instant::now();
        Self {
            epoch,
            next_epoch,
            capacity,
            registrations: 0,
            disposal_events: 0,
            omitted_observations: 0,
            rows: Vec::new(),
            global_began,
            global_ended,
            global: Value::new(rc, current),
            global_highwater: (rc == ffi::SQLITE_OK && highwater >= 0).then_some(highwater),
        }
    }

    pub(super) fn row_state(&self, row: &Row) -> &'static str {
        if row.disposed {
            "instrumentation_disposed"
        } else {
            match row.sample {
                Some(sample) if sample.epoch == self.epoch => "sampled_epoch",
                Some(_) => "stale",
                None => "never_sampled",
            }
        }
    }

    pub fn emit(self) {
        let sampled = self
            .rows
            .iter()
            .filter(|row| self.row_state(row) == "sampled_epoch")
            .count();
        let stale = self
            .rows
            .iter()
            .filter(|row| self.row_state(row) == "stale")
            .count();
        let never = self
            .rows
            .iter()
            .filter(|row| self.row_state(row) == "never_sampled")
            .count();
        // Compute at each event's creation, after output-job queueing and any
        // previous subscriber blocking. The current event's sink completion
        // happens later and is deliberately not claimed by this age.
        let global_emit = Instant::now();
        tracing::warn!(
            epoch = self.epoch, next_requested_epoch = self.next_epoch,
            registry_capacity = self.capacity, registered_rows = self.rows.len(),
            observation_registrations = self.registrations,
            instrumentation_disposal_events = self.disposal_events,
            omitted_observations = self.omitted_observations,
            sampled_epoch_rows = sampled, stale_rows = stale, never_sampled_rows = never,
            complete_pool_census = false, physical_connection_count = "unknown",
            global_memory_rc = self.global.rc,
            global_reported_current_bytes = ?self.global.bytes,
            global_reported_highwater_bytes = ?self.global_highwater,
            global_sample_age_us = super::telemetry::micros(global_emit.saturating_duration_since(self.global_ended)),
            global_sample_duration_us = super::telemetry::micros(self.global_ended.duration_since(self.global_began)),
            sample_age_scope = "event_creation_not_sink_completion; global_and_connection_reads_are_separate_intervals",
            memstatus_availability = "not_verified_zero_is_not_proof",
            scope = "sqlite_library_instance_process_aggregate_not_RSS_not_pool_total; connection_metrics_overlap",
            "SQLite memory diagnostic summary"
        );
        for row in &self.rows {
            let state = self.row_state(row);
            let values = row.sample.map(|sample| sample.values);
            let row_emit = Instant::now();
            tracing::warn!(
                connection_sequence = row.sequence, requested_epoch = self.epoch,
                sampled_epoch = ?row.sample.map(|sample| sample.epoch), snapshot_state = state,
                sample_age_us = ?row.sample.map(|sample| super::telemetry::micros(row_emit.saturating_duration_since(sample.ended))),
                sample_duration_us = ?row.sample.map(|sample| super::telemetry::micros(sample.ended.duration_since(sample.began))),
                cache_rc = ?values.map(|values| values[0].rc), cache_approximate_bytes = ?values.and_then(|values| values[0].bytes),
                cache_shared_rc = ?values.map(|values| values[1].rc), cache_shared_approximate_bytes = ?values.and_then(|values| values[1].bytes),
                schema_rc = ?values.map(|values| values[2].rc), schema_approximate_bytes = ?values.and_then(|values| values[2].bytes),
                statement_rc = ?values.map(|values| values[3].rc), statement_approximate_bytes = ?values.and_then(|values| values[3].bytes),
                scope = "existing_connection_cleanup_tail_diesel_transaction_depth_none; sequential_status_reads_not_RSS; shared_schema_may_repeat",
                "SQLite connection memory diagnostic"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel::connection::SimpleConnection;
    use diesel::prelude::*;

    diesel::table! {
        memory_probe (id) {
            id -> Integer,
        }
    }

    #[test]
    fn only_exact_explicit_opt_in_is_enabled() {
        assert!(!opted_in(None));
        for value in ["", "0", "true", " 1", "1 "] {
            assert!(!opted_in(Some(OsStr::new(value))));
        }
        assert!(opted_in(Some(OsStr::new("1"))));
    }

    #[test]
    fn real_connection_bytes_and_unsupported_status_do_not_change_transactions() {
        let mut connection = SqliteConnection::establish(":memory:").unwrap();
        connection.batch_execute("CREATE TABLE memory_probe(id INTEGER PRIMARY KEY); INSERT INTO memory_probe VALUES(7);").unwrap();
        let rows = memory_probe::table
            .select(memory_probe::id)
            .load::<i32>(&mut connection)
            .unwrap();
        assert_eq!(rows, [7]);
        let sampled = sample(&mut connection, 1);
        for value in sampled.values {
            assert_eq!(value.rc, ffi::SQLITE_OK);
            assert!(value.bytes.unwrap() > 0);
        }
        // SAFETY: only a status read on the callback's live borrowed handle.
        let unsupported = unsafe { connection.with_raw_connection(|db| db_value(db, i32::MAX)) };
        assert_ne!(unsupported.rc, ffi::SQLITE_OK);
        assert!(unsupported.bytes.is_none());
        connection
            .transaction::<_, diesel::result::Error, _>(|connection| {
                // SAFETY: read autocommit only; no pointer escapes the callback.
                let before =
                    unsafe { connection.with_raw_connection(|db| ffi::sqlite3_get_autocommit(db)) };
                let _ = sample(connection, 2);
                // SAFETY: as above.
                let after =
                    unsafe { connection.with_raw_connection(|db| ffi::sqlite3_get_autocommit(db)) };
                assert_eq!((before, after), (0, 0));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            memory_probe::table
                .count()
                .get_result::<i64>(&mut connection)
                .unwrap(),
            1
        );
    }
}
