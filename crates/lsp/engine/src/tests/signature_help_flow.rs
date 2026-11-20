use gen_lsp_types::{ActiveParameter, ParameterInformationLabel};
use rg_lsp_proto::SignatureHelpClientCapabilities;

use super::utils::LspEngineFixture;

#[tokio::test]
async fn signature_help_tracks_written_arguments_in_unsaved_calls() {
    let fixture = LspEngineFixture::initialized(
        r#"
        //- /Cargo.toml
        [package]
        name = "lsp_signature_help"
        version = "0.1.0"
        edition = "2024"

        //- /src/lib.rs
        fn demo() {}
        "#,
    )
    .await;
    fixture.did_open_saved("src/lib.rs", 1).await;
    let dirty = fixture
        .did_change_full(
            "src/lib.rs",
            2,
            r#"
fn send<T>(payload: T, retry: bool) -> bool { retry }
fn 𐐀(_: &str, _: &str) {}
unsafe extern "C" {
    fn print(count: u8, ...);
}
struct Client;
struct Pair<T>(T, bool);
enum Event<T> { Received(T, bool) }
impl Client {
    fn send(&self, payload: u8, retry: bool) {}
    fn ping(&self) {}
}
fn demo(client: Client, make_client: fn() -> Client) {
    send((1, $tuple$2), $after_tuple$true);
    send(𐐀("", $inner$"")$after_inner$, $outer$true);
    send(#[cfg(any())] (), $cfg$42, true);
    send(, $missing_argument$true);
    Client::send($explicit_self$&client, 1, false);
    make_client().send(1, $inferred_receiver$false);
    client.ping($no_arguments$);
    unsafe { print(2, 42, $variadic$43); }
}
fn unfinished_function() {
    send($after_open$
}
fn unfinished_pointer(callback: &fn(u8, bool) -> u16) {
    callback($pointer$
}
fn unfinished_closure() {
    let callback = |_: u8, value| value;
    callback(1, $closure$
}
fn unfinished_tuple_struct() {
    Pair::<u8>(1, $tuple_struct$
}
fn unfinished_tuple_variant() {
    let make = Event::Received;
    make(1_u16, $tuple_variant$
}
fn unfinished_local_constructor() {
    type Payload = u16;
    struct Local(Payload, bool);
    Local($local_constructor$
}
fn unfinished_method(client: Client) {
    client.send(1, $after_comma$
"#,
        )
        .await;

    let free = "fn send<T>(payload: T, retry: bool) -> bool";
    let method = "fn send(&self, payload: u8, retry: bool)";
    for (marker, label, index, parameter) in [
        ("after_open", free, 0, "payload: T"),
        ("tuple", free, 0, "payload: T"),
        ("after_tuple", free, 1, "retry: bool"),
        ("after_inner", free, 0, "payload: T"),
        ("outer", free, 1, "retry: bool"),
        ("cfg", free, 0, "payload: T"),
        ("missing_argument", free, 1, "retry: bool"),
        ("explicit_self", method, 0, "&self"),
        ("inferred_receiver", method, 1, "retry: bool"),
        ("after_comma", method, 1, "retry: bool"),
        ("variadic", "fn print(count: u8, ...)", 1, "..."),
        ("pointer", "fn(u8, bool) -> u16", 0, "u8"),
        ("closure", "|u8, _| -> _", 1, "_"),
        ("tuple_struct", "Pair(u8, bool) -> Pair<u8>", 1, "bool"),
        (
            "tuple_variant",
            "Event::Received(u16, bool) -> Event<u16>",
            1,
            "bool",
        ),
        ("local_constructor", "Local(u16, bool) -> Local", 0, "u16"),
    ] {
        let help = fixture
            .signature_help(&dirty, marker, SignatureHelpClientCapabilities::default())
            .await
            .unwrap_or_else(|| panic!("signature help should be available at {marker}"));
        let [signature] = help.signatures.as_slice() else {
            panic!("expected one signature at {marker}");
        };
        assert_eq!(signature.label, label, "{marker}");
        assert_eq!(
            help.active_parameter,
            Some(ActiveParameter::Int(index)),
            "{marker}"
        );
        assert_eq!(
            signature.parameters.as_ref().expect("parameter labels")[index as usize].label,
            ParameterInformationLabel::String(parameter.to_owned()),
            "{marker}"
        );
    }

    let client = SignatureHelpClientCapabilities {
        label_offset_support: true,
        no_active_parameter_support: true,
    };
    let inner = fixture
        .signature_help(&dirty, "inner", client)
        .await
        .expect("nested signature help should be available");
    assert_eq!(inner.signatures[0].label, "fn 𐐀(_: &str, _: &str)");
    assert_eq!(inner.active_parameter, Some(ActiveParameter::Int(1)));
    let parameters = inner.signatures[0]
        .parameters
        .as_ref()
        .expect("parameter offsets");
    assert_eq!(
        parameters[0].label,
        ParameterInformationLabel::Tuple((6, 13))
    );
    assert_eq!(
        parameters[1].label,
        ParameterInformationLabel::Tuple((15, 22))
    );

    let ping = fixture
        .signature_help(&dirty, "no_arguments", client)
        .await
        .expect("zero-argument signature should be available");
    assert_eq!(ping.signatures[0].label, "fn ping(&self)");
    assert_eq!(ping.active_parameter, Some(ActiveParameter::Null));
    assert!(
        ping.signatures[0]
            .parameters
            .as_ref()
            .is_some_and(Vec::is_empty)
    );

    fixture.shutdown().await;
}
