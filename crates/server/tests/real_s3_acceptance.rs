//! Opt-in integration against an actual S3-compatible server, never a mock.
//! Set OFFICE_S3_ENDPOINT, OFFICE_S3_BUCKET, OFFICE_S3_ACCESS_KEY,
//! OFFICE_S3_SECRET_KEY, and optionally OFFICE_S3_REGION before running ignored tests.
use contrix_server::services::{StorageBackend, s3_storage::S3Storage};

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("real S3 acceptance requires {name}"))
}

#[tokio::test]
#[ignore = "requires a real, independently running S3-compatible service and disposable bucket"]
async fn real_s3_authentication_binary_roundtrip_encoded_keys_and_delete() {
    let endpoint = required("OFFICE_S3_ENDPOINT");
    let bucket = required("OFFICE_S3_BUCKET");
    let access = required("OFFICE_S3_ACCESS_KEY");
    let secret = required("OFFICE_S3_SECRET_KEY");
    let region = std::env::var("OFFICE_S3_REGION").unwrap_or_else(|_| "us-east-1".into());
    let storage = S3Storage::new(&bucket, &region, &access, &secret, &endpoint);
    storage
        .head_bucket()
        .await
        .expect("authenticated bucket HEAD");
    let unauthorized = S3Storage::new(&bucket, &region, &access, "invalid-secret", &endpoint);
    assert!(
        unauthorized.head_bucket().await.is_err(),
        "server must verify the signature"
    );
    let prefix = format!("office-acceptance/{}/", uuid::Uuid::new_v4());
    let keys = [
        format!("{prefix}binary.bin"),
        format!("{prefix}空 格+%#?.bin"),
    ];
    let payload: Vec<u8> = (0..65_537).map(|index| (index % 251) as u8).collect();
    // Always attempt cleanup if an object operation or equality check fails.
    let checks = async {
        for key in &keys {
            if storage
                .head_object(key)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err("fresh UUID object already exists".into());
            }
            storage
                .put_object(key, &payload, "application/octet-stream")
                .await
                .map_err(|error| error.to_string())?;
            if !storage
                .head_object(key)
                .await
                .map_err(|error| error.to_string())?
            {
                return Err("HEAD did not find the uploaded object".into());
            }
            if storage
                .get_object(key)
                .await
                .map_err(|error| error.to_string())?
                != payload
            {
                return Err("binary GET did not match PUT bytes".into());
            }
        }
        Ok::<(), String>(())
    }
    .await;
    let mut cleanup_errors = Vec::new();
    for key in &keys {
        if let Err(error) = storage.delete_object(key).await {
            cleanup_errors.push(error.to_string());
        }
        match storage.head_object(key).await {
            Ok(false) => {}
            Ok(true) => cleanup_errors.push("deleted object remains visible to HEAD".into()),
            Err(error) => cleanup_errors.push(error.to_string()),
        }
        if storage.get_object(key).await.is_ok() {
            cleanup_errors.push("deleted object remains readable".into());
        }
        if let Err(error) = storage.delete_object(key).await {
            cleanup_errors.push(format!("idempotent DELETE: {error}"));
        }
    }
    checks.unwrap();
    assert!(
        cleanup_errors.is_empty(),
        "cleanup errors: {cleanup_errors:?}"
    );
    println!(
        "{}",
        serde_json::json!({"scenario":"real_s3_roundtrip", "endpoint":endpoint,
        "region":region,"objects":keys.len(),"bytes_per_object":payload.len(),
        "signature_rejection":true,"roundtrip":true,"delete":true})
    );
}

#[tokio::test]
#[ignore = "requires a real, independently running S3-compatible service and disposable bucket"]
async fn real_s3_same_named_concurrent_saves_have_independent_objects() {
    let endpoint = required("OFFICE_S3_ENDPOINT");
    let bucket = required("OFFICE_S3_BUCKET");
    let access = required("OFFICE_S3_ACCESS_KEY");
    let secret = required("OFFICE_S3_SECRET_KEY");
    let region = std::env::var("OFFICE_S3_REGION").unwrap_or_else(|_| "us-east-1".into());
    let storage = StorageBackend::S3(S3Storage::new(
        &bucket, &region, &access, &secret, &endpoint,
    ));
    storage
        .check_ready()
        .await
        .expect("real S3 bucket is ready");
    let owner = uuid::Uuid::new_v4();
    let name = "same-upload.bin";
    let first_bytes: Vec<u8> = (0..65_537).map(|index| (index % 251) as u8).collect();
    let second_bytes: Vec<u8> = first_bytes.iter().map(|&byte| 255 - byte).collect();

    // Exercise the production UUID allocation in StorageBackend::save_file,
    // rather than assigning two distinct object keys in the test itself.
    let (first, second) = tokio::join!(
        storage.save_file(&owner, name, &first_bytes),
        storage.save_file(&owner, name, &second_bytes)
    );
    // Keep both results until cleanup, including when only one save succeeds.
    let checks = async {
        let first = first.as_ref().map_err(|error| error.to_string())?;
        let second = second.as_ref().map_err(|error| error.to_string())?;
        if first == second {
            return Err("concurrent same-owner/name saves returned the same object key".into());
        }
        if storage
            .read_file(first)
            .await
            .map_err(|error| error.to_string())?
            != first_bytes
        {
            return Err("first object's bytes were overwritten".into());
        }
        if storage
            .read_file(second)
            .await
            .map_err(|error| error.to_string())?
            != second_bytes
        {
            return Err("second object's bytes were overwritten".into());
        }
        storage
            .delete_file(first)
            .await
            .map_err(|error| error.to_string())?;
        if storage.read_file(first).await.is_ok() {
            return Err("deleted first object remains readable".into());
        }
        if storage
            .read_file(second)
            .await
            .map_err(|error| error.to_string())?
            != second_bytes
        {
            return Err("deleting the first object changed the second object".into());
        }
        Ok::<(), String>(())
    }
    .await;

    let mut cleanup_errors = Vec::new();
    for path in [&first, &second]
        .into_iter()
        .filter_map(|saved| saved.as_ref().ok())
    {
        if let Err(error) = storage.delete_file(path).await {
            cleanup_errors.push(error.to_string());
        }
        if storage.read_file(path).await.is_ok() {
            cleanup_errors.push("test object remains readable after cleanup".into());
        }
    }
    checks.unwrap();
    assert!(
        cleanup_errors.is_empty(),
        "cleanup errors: {cleanup_errors:?}"
    );
    println!(
        "{}",
        serde_json::json!({
            "scenario": "real_s3_same_named_concurrent_saves",
            "endpoint": endpoint,
            "region": region,
            "objects": 2,
            "bytes_per_object": first_bytes.len(),
            "high_level_save_file": true,
            "distinct_keys": true,
            "independent_roundtrips": true,
            "delete_isolation": true,
            "cleanup": true
        })
    );
}
