//! Exercise the real semantic mount validation, not just builder construction.
use crate::state::AuthState;
use glory::prelude::*;

fn assert_mounts<W: Widget>(
    name: &str,
    mut factory: impl FnMut(Scope, Cage<AuthState>) -> W + 'static,
) {
    let app = App::memory();
    app.mount(widget(move |scope| {
        let auth = scope.cage(AuthState::default());
        factory(scope.clone(), auth).build(scope)
    }))
    .unwrap_or_else(|error| panic!("{name} must pass semantic mount validation: {error}"));
    app.flush().unwrap();
    assert!(!app.tree().roots().is_empty());
    app.clear().unwrap();
}

#[test]
fn navbar_icon_buttons_mount() {
    assert_mounts("Navbar", super::navbar::Navbar);
}

#[test]
fn sidebar_icon_buttons_mount() {
    assert_mounts("Sidebar", super::sidebar::Sidebar);
}

#[test]
fn dashboard_quick_action_buttons_mount() {
    assert_mounts("Dashboard", crate::pages::Dashboard);
}

#[test]
fn mobile_icon_buttons_mount() {
    assert_mounts("MobileBar", super::mobile_bar::MobileBar);
}

#[test]
fn file_toolbar_icon_buttons_mount() {
    assert_mounts("FileManager", crate::pages::FileManager);
}
