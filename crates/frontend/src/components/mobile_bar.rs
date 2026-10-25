use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn MobileBar(_scope: Scope, _auth: Cage<AuthState>) -> impl Widget {
    row().class("mobile-bar").fill((
        anchor("Home", "/dashboard"),
        anchor("Files", "/files"),
        anchor("Profile", "/profile"),
    ))
}
