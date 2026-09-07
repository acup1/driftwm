//! `zwp_virtual_keyboard_v1` (`src/protocols/virtual_keyboard.rs`, vendored
//! from smithay with one addition): a virtual key is resolved against the
//! virtual keyboard's *own* uploaded keymap and modifier state, and a bound
//! combo runs the compositor action instead of reaching the focused client
//! (press and its paired release both swallowed). Everything unbound follows
//! smithay: the focused client gets the virtual keyboard's keymap before its
//! keys, and a physical key afterwards must find the seat's own keymap
//! restored first.

use smithay::input::keyboard::xkb;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::{
    self, ZwpVirtualKeyboardV1,
};

use super::client::{ClientId, KeyboardEvent};
use super::input_backend::{key_press, key_release};
use super::{Fixture, config, keyboard_focus, map_window, server_surface, window_by_app_id};

/// Evdev keycode the virtual keyboard presses in most scenarios here.
const KEY_A: u32 = 30;
/// Evdev keycode for `=`, whose "equal" keysym backs the bound combo in
/// [`a_virtual_key_matching_a_compositor_binding_runs_the_action_and_never_reaches_the_client`].
const KEY_EQUAL: u32 = 13;
/// A harmless physical keycode, distinct from `KEY_A` so a test mixing both
/// paths never confuses one for the other.
const KEY_B: u32 = 48;

/// Compile an XKB keymap for `layout` under the default ("evdev") rules, the
/// way a real on-screen keyboard would build the one it uploads.
fn compile_keymap(layout: &str) -> xkb::Keymap {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        layout,
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("compile a keymap")
}

/// Map a focused window for `a`, then have `b`'s virtual keyboard upload a
/// `de` keymap — deliberately not the fixture's default `us` seat layout, so
/// a client actually holding it is observable — and press + release `KEY_A`.
/// Returns the virtual keyboard and the keymap text it uploaded.
fn press_a_through_virtual_keyboard(
    f: &mut Fixture,
    a: ClientId,
    b: ClientId,
) -> (ZwpVirtualKeyboardV1, String) {
    map_window(f, a, "typist", (400, 300));
    assert_eq!(
        keyboard_focus(f),
        Some(server_surface(&window_by_app_id(f, "typist").unwrap())),
        "precondition: the mapped window holds keyboard focus"
    );
    f.client(a).drain_keyboard_events();

    let vk = f.client(b).create_virtual_keyboard();
    let text = compile_keymap("de").get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    f.client(b).virtual_keyboard_keymap(&vk, &text);
    f.roundtrip(b);

    f.client(b).virtual_keyboard_key(&vk, 1, KEY_A, true);
    f.client(b).virtual_keyboard_key(&vk, 2, KEY_A, false);
    f.roundtrip(b);
    f.roundtrip(a);

    (vk, text)
}

#[test]
fn a_virtual_key_reaches_the_focused_window_with_the_virtual_keyboards_keymap_first() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let a = f.add_client();
    let b = f.add_client();

    let (_vk, text) = press_a_through_virtual_keyboard(&mut f, a, b);

    assert_eq!(
        f.client(a).drain_keyboard_events(),
        vec![
            // A keymap change is always followed by a modifiers event to
            // resync the client's state — here the virtual keyboard's own
            // default (nothing depressed).
            KeyboardEvent::Keymap(text),
            KeyboardEvent::Modifiers { mods_depressed: 0 },
            KeyboardEvent::Key {
                key: KEY_A,
                state: 1
            },
            KeyboardEvent::Key {
                key: KEY_A,
                state: 0
            },
        ],
        "the client must see the virtual keyboard's keymap exactly once, then \
         the press and the release — a second key must not re-send a keymap \
         the client already holds"
    );
}

#[test]
fn a_virtual_key_matching_a_compositor_binding_runs_the_action_and_never_reaches_the_client() {
    let mut f = Fixture::with_config(config(
        r#"
        [keybindings]
        "ctrl+alt+equal" = "zoom-in"
        "#,
    ));
    f.add_output(1, (1920, 1080));
    let a = f.add_client();
    map_window(&mut f, a, "typist", (400, 300));
    assert_eq!(
        keyboard_focus(&mut f),
        Some(server_surface(&window_by_app_id(&mut f, "typist").unwrap())),
        "precondition: the mapped window holds keyboard focus"
    );
    f.client(a).drain_keyboard_events();

    let b = f.add_client();
    let vk = f.client(b).create_virtual_keyboard();
    let keymap = compile_keymap("us");
    let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    f.client(b).virtual_keyboard_keymap(&vk, &text);
    f.roundtrip(b);

    let ctrl = keymap.mod_get_index(xkb::MOD_NAME_CTRL);
    let alt = keymap.mod_get_index(xkb::MOD_NAME_ALT);
    let mask = (1u32 << ctrl) | (1u32 << alt);
    f.client(b).virtual_keyboard_modifiers(&vk, mask);
    f.roundtrip(b);

    f.client(b).virtual_keyboard_key(&vk, 1, KEY_EQUAL, true);
    f.client(b).virtual_keyboard_key(&vk, 2, KEY_EQUAL, false);
    f.roundtrip(b);
    f.roundtrip(a);

    assert!(
        f.state().zoom_target().is_some(),
        "a virtual key matching a compositor binding must run its action"
    );
    assert!(
        !f.client(a)
            .keyboard_events()
            .iter()
            .any(|e| matches!(e, KeyboardEvent::Key { .. })),
        "a key a compositor binding consumes must never reach the focused \
         client, neither the press nor its paired release: {:?}",
        f.client(a).keyboard_events()
    );
}

#[test]
fn a_physical_key_after_virtual_typing_restores_the_seat_keymap_first() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let a = f.add_client();
    let b = f.add_client();

    let (_vk, virtual_text) = press_a_through_virtual_keyboard(&mut f, a, b);
    // The virtual delivery itself is pinned by the scenario above; drop it so
    // only what the physical key produces is left to inspect.
    f.client(a).drain_keyboard_events();

    key_press(&mut f, KEY_B);
    key_release(&mut f, KEY_B);
    f.roundtrip(a);

    let events = f.client(a).drain_keyboard_events();
    let Some(KeyboardEvent::Keymap(seat_text)) = events.first() else {
        panic!("expected the seat's keymap to go out before the physical key, got {events:?}");
    };
    assert_ne!(
        seat_text, &virtual_text,
        "a physical key must find the seat's own keymap restored, not the one \
         the virtual keyboard left the client holding"
    );

    let key_events: Vec<&KeyboardEvent> = events
        .iter()
        .filter(|e| matches!(e, KeyboardEvent::Key { .. }))
        .collect();
    assert_eq!(
        key_events,
        vec![
            &KeyboardEvent::Key {
                key: KEY_B,
                state: 1
            },
            &KeyboardEvent::Key {
                key: KEY_B,
                state: 0
            },
        ],
        "the physical press and release must still reach the client, after \
         the keymap reset"
    );
}

/// This test kills client `b` after catching the error — a protocol error on a
/// live object leaves its `WaylandSource` errored but still registered, so any
/// later `Fixture::roundtrip`/`dispatch`, even for a different client, would
/// panic on it (see `session_lock.rs`'s `expect_lock_surface_error`).
#[test]
fn key_before_keymap_is_a_protocol_error() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let b = f.add_client();
    f.roundtrip(b);
    let vk = f.client(b).create_virtual_keyboard();

    f.client(b).virtual_keyboard_key(&vk, 1, KEY_A, true);
    f.pump(10);

    let error = f
        .client(b)
        .protocol_error()
        .expect("`key` before `keymap` must be a protocol error");
    assert_eq!(error.object_interface, "zwp_virtual_keyboard_v1");
    assert_eq!(
        error.code,
        zwp_virtual_keyboard_v1::Error::NoKeymap as u32,
        "wrong error code, got: {}",
        error.message
    );

    f.kill_client(b);
}

#[test]
fn a_modifiers_request_reaches_the_focused_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let a = f.add_client();
    map_window(&mut f, a, "typist", (400, 300));
    assert_eq!(
        keyboard_focus(&mut f),
        Some(server_surface(&window_by_app_id(&mut f, "typist").unwrap())),
        "precondition: the mapped window holds keyboard focus"
    );
    f.client(a).drain_keyboard_events();

    let b = f.add_client();
    let vk = f.client(b).create_virtual_keyboard();
    let keymap = compile_keymap("de");
    f.client(b)
        .virtual_keyboard_keymap(&vk, &keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1));
    f.roundtrip(b);

    let ctrl = keymap.mod_get_index(xkb::MOD_NAME_CTRL);
    let mask = 1u32 << ctrl;
    f.client(b).virtual_keyboard_modifiers(&vk, mask);
    f.roundtrip(b);
    f.roundtrip(a);

    assert!(
        f.client(a)
            .drain_keyboard_events()
            .contains(&KeyboardEvent::Modifiers {
                mods_depressed: mask
            }),
        "a `modifiers` request must reach the focused window's wl_keyboard, \
         possibly preceded by the keymap on first contact"
    );
}

/// Destroying a virtual keyboard must free its compositor-side bookkeeping —
/// checked here directly, and again by the fixture's own teardown baseline
/// (`debug_counters`' `virtual_kb_bindings` entry is
/// `VirtualKeyboardBindings::keyboard_count`), which this scenario does not
/// opt out of.
#[test]
fn destroying_a_virtual_keyboard_frees_its_bookkeeping() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let a = f.add_client();
    let b = f.add_client();

    let (vk, _text) = press_a_through_virtual_keyboard(&mut f, a, b);
    assert_eq!(
        f.state().virtual_kb_bindings.keyboard_count(),
        1,
        "precondition: the manager registered the virtual keyboard"
    );

    f.client(b).virtual_keyboard_destroy(&vk);
    f.roundtrip(b);

    assert_eq!(
        f.state().virtual_kb_bindings.keyboard_count(),
        0,
        "destroying the virtual keyboard must free its compositor-side bookkeeping"
    );
}
