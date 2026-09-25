//! Terminal injection: text chosen by the other side must not be able to
//! drive the terminal, disguise itself, or hide what it is (ADR-0034).
//!
//! These run the real engine and the real prompt desk, with a screen the test
//! can read back, so what is asserted is what a person would actually see.

use std::collections::HashSet;
use std::time::Duration;

use beam::cli::desk::{DeskPrompt, testing};
use beam::identity::{Identity, KnownPeers, Peer, encode_public_key};
use beam::pairing::choose_name;
use beam::transfer::frame::{read_message, write_message};
use beam::transfer::message::{Cancel, Message, RejectReason, TransferId, TransferRequest};
use beam::transfer::{
    ReceiveOptions, SendOptions, SilentReporter, TransferError, receive_file, send_file, sha256_hex,
};

const PATIENCE: Duration = Duration::from_secs(10);

struct Setup {
    _tmp: tempfile::TempDir,
    sender: Identity,
    known: KnownPeers,
    options: ReceiveOptions,
    files: std::path::PathBuf,
    out: std::path::PathBuf,
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let sender = Identity::generate("alice").unwrap();
    let mut known = KnownPeers::with_header();
    known
        .add(Peer::new("alice", sender.verifying_key()))
        .unwrap();
    let out = tmp.path().join("out");
    let files = tmp.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    let mut options = ReceiveOptions::new(&out, tmp.path().join("work"));
    options.accept_timeout = Duration::from_secs(10);
    Setup {
        _tmp: tmp,
        sender,
        known,
        options,
        files,
        out,
    }
}

/// Sends a file whose *name* is `name`, through the real desk; answers the
/// prompt with `y` once it appears. Returns what the screen showed and the
/// receiver's result.
async fn send_named(
    s: &Setup,
    name: &str,
) -> (
    String,
    Result<beam::transfer::ReceiveSummary, TransferError>,
) {
    let bytes = b"contents".to_vec();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let request = Message::TransferRequest(TransferRequest {
        transfer_id: TransferId::generate().unwrap(),
        sender_public_key: encode_public_key(&s.sender.verifying_key()),
        file_name: name.to_string(),
        size: bytes.len() as u64,
        chunk_size: bytes.len() as u32,
        chunk_count: 1,
        file_sha256: sha256_hex(&bytes),
    });

    let (desk, typed, screen) = testing::desk();
    let prompt = DeskPrompt::new(desk, Duration::from_secs(10));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let answering = {
        let screen = screen.clone();
        let done = std::sync::Arc::clone(&done);
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + PATIENCE;
            while std::time::Instant::now() < deadline
                && !done.load(std::sync::atomic::Ordering::SeqCst)
            {
                if screen.text().contains("Accept? [y/N]") {
                    let _ = typed.send("y".into());
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };

    let peer = tokio::spawn(async move {
        write_message(&mut client, &request).await.unwrap();
        // Play the rest of an honest sender if accepted.
        if let Ok(Message::Accept(_)) = read_message(&mut client).await {
            write_message(
                &mut client,
                &Message::ChunkStart(beam::transfer::message::ChunkStart {
                    index: 0,
                    len: bytes.len() as u32,
                    sha256: sha256_hex(&bytes),
                }),
            )
            .await
            .unwrap();
            write_message(&mut client, &Message::ChunkData { index: 0, bytes })
                .await
                .unwrap();
            let _ = read_message(&mut client).await; // ACK
            let transfer_id = TransferId::from_bytes([0; 16]);
            let _ = write_message(
                &mut client,
                &Message::Complete(beam::transfer::message::Complete {
                    transfer_id,
                    final_name: None,
                }),
            )
            .await;
            while read_message(&mut client).await.is_ok() {}
        }
    });

    let result = receive_file(
        server,
        &s.known,
        &s.options,
        prompt,
        &mut SilentReporter,
        &mut HashSet::new(),
    )
    .await;
    let _ = peer.await;
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = answering.join();
    (screen.text(), result)
}

// ------------------------------------------------------------ file names

#[tokio::test]
async fn a_name_with_escape_sequences_is_refused_before_the_prompt() {
    let s = setup();
    for name in [
        "\u{1B}[2J\u{1B}[Hclean.txt",
        "\u{1B}]0;pwned\u{07}title.txt",
        "fine.txt\r\u{1B}[Kfake.txt",
        "\u{9B}31mc1.txt",
    ] {
        let (screen, result) = send_named(&s, name).await;
        assert!(
            matches!(result, Err(TransferError::Name(_))),
            "{name:?}: {result:?}"
        );
        assert!(
            !screen.contains("Incoming file"),
            "{name:?} reached the prompt"
        );
        assert!(!screen.contains('\u{1B}'));
    }
}

/// M6 answer 1: a name with U+202E is refused, and never shown.
#[tokio::test]
async fn a_name_with_a_bidi_override_is_refused_before_the_prompt() {
    let s = setup();
    let (screen, result) = send_named(&s, "invoice\u{202E}fdp.exe").await;
    assert!(
        matches!(
            result,
            Err(TransferError::Name(beam::transfer::NameError::BidiControl(
                '\u{202E}'
            )))
        ),
        "{result:?}"
    );
    assert!(!screen.contains("Incoming file"));
    assert!(std::fs::read_dir(&s.out).map(|d| d.count()).unwrap_or(0) == 0);
}

/// M6 answer 1: Thai with vowels and tone marks passes through unchanged —
/// in the prompt and on disk.
#[tokio::test]
async fn a_thai_name_is_shown_and_saved_unchanged() {
    let s = setup();
    let thai = "รายงานประจำปี_ฉบับที่๒.pdf";
    let (screen, result) = send_named(&s, thai).await;
    let summary = result.expect("a Thai name is a fine name");
    assert_eq!(summary.final_name, thai);
    assert!(screen.contains(thai), "{screen}");
    assert!(s.out.join(thai).exists());
}

/// M6 answer 1: an emoji with ZWJ is accepted, saved as sent, and shown with
/// the joiner visible.
#[tokio::test]
async fn an_emoji_name_with_zwj_is_accepted_and_shown_safely() {
    let s = setup();
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}.jpg";
    let (screen, result) = send_named(&s, family).await;
    assert_eq!(result.expect("accepted").final_name, family);
    assert!(
        screen.contains("\u{1F468}<U+200D>\u{1F469}<U+200D>\u{1F467}.jpg"),
        "{screen}"
    );
    assert!(s.out.join(family).exists(), "saved under the name as sent");
}

#[tokio::test]
async fn a_zero_width_space_in_a_name_is_made_visible_in_the_prompt() {
    let s = setup();
    let (screen, result) = send_named(&s, "ข่าว\u{200B}ดี.txt").await;
    result.expect("accepted");
    assert!(screen.contains("ข่าว<U+200B>ดี.txt"), "{screen}");
}

/// M6 answer 5: a name long enough that cutting at the end would hide `.exe`
/// is cut in the middle instead.
#[tokio::test]
async fn a_long_name_shows_its_real_extension_in_the_prompt() {
    let s = setup();
    let name = format!("Quarterly_report{}.pdf.exe", "_final".repeat(20));
    let (screen, result) = send_named(&s, &name).await;
    result.expect("a long name within the byte limit is accepted");
    let line = screen
        .lines()
        .find(|l| l.trim_start().starts_with("File"))
        .unwrap_or_else(|| panic!("no File line in {screen}"));
    assert!(line.trim_end().ends_with(".pdf.exe"), "{line}");
    assert!(line.contains('…'), "{line}");
}

// ------------------------------------------------------------- hints, text

/// A hostile host-name hint becomes a plain nickname before it is shown or
/// saved.
#[test]
fn a_hostile_hint_becomes_a_plain_name() {
    let known = KnownPeers::with_header();
    let key = Identity::generate("x").unwrap().verifying_key();
    for hint in [
        "\u{1B}[31mevil\u{1B}[0m",
        "laptop\u{202E}exe.pdf",
        "a\rb",
        "\u{1B}]0;title\u{07}pc",
        "ข่าว\u{200B}",
    ] {
        let name = choose_name(&known, Some(hint), &key);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "{hint:?} became {name:?}"
        );
        beam::identity::validate_name(&name).unwrap();
    }
}

/// The pairing prompt shows the chosen name through the sanitizer too.
#[test]
fn the_pairing_prompt_cannot_be_driven_by_the_name() {
    let (desk, typed, screen) = testing::desk();
    let request = beam::pairing::ConfirmRequest {
        role: beam::pairing::Role::Waiter,
        name: "x\u{1B}[2J\u{202E}".into(),
        peer_fingerprint: beam::identity::Fingerprint::of(
            &Identity::generate("a").unwrap().verifying_key(),
        ),
        own_fingerprint: beam::identity::Fingerprint::of(
            &Identity::generate("b").unwrap().verifying_key(),
        ),
    };
    let asking = std::thread::spawn(move || {
        let mut prompt = DeskPrompt::new(desk, Duration::from_secs(5));
        beam::pairing::Confirm::confirm(&mut prompt, &request)
    });
    screen.wait_for("Type \"yes\"", PATIENCE);
    typed.send("no".into()).unwrap();
    assert!(!asking.join().unwrap().unwrap());
    let text = screen.text();
    assert!(!text.contains('\u{1B}'), "{text:?}");
    assert!(text.contains("<U+202E>"), "{text}");
}

/// A CANCEL reason is the peer's text: it reaches the error message only
/// through the sanitizer.
#[tokio::test]
async fn a_cancel_reason_cannot_carry_escapes() {
    let s = setup();
    let path = s.files.join("a.bin");
    std::fs::write(&path, [1u8; 10]).unwrap();
    let (mut ours, mut peer) = tokio::io::duplex(64 * 1024);
    let options = SendOptions::new(&path, encode_public_key(&s.sender.verifying_key()));
    let sending =
        tokio::spawn(async move { send_file(&mut ours, &options, &mut SilentReporter).await });
    let Ok(Message::TransferRequest(request)) = read_message(&mut peer).await else {
        panic!()
    };
    write_message(
        &mut peer,
        &Message::Cancel(Cancel {
            transfer_id: request.transfer_id,
            reason: "\u{1B}[2J\u{1B}]0;owned\u{07}bye\rSHA256:fake".into(),
        }),
    )
    .await
    .unwrap();
    let err = sending.await.unwrap().unwrap_err();
    let shown = err.to_string();
    assert!(
        !shown.contains('\u{1B}') && !shown.contains('\r'),
        "{shown:?}"
    );
    assert!(shown.contains("bye"), "{shown}");
}

/// A REJECT reason is an enum, so it cannot carry text at all; the message
/// the sender prints is ours.
#[test]
fn reject_reasons_are_not_free_text() {
    let parsed: Result<beam::transfer::message::Reject, _> = serde_json::from_str(
        r#"{"transfer_id":"00112233445566778899aabbccddeeff","reason":"\u001b[2Jdeclined"}"#,
    );
    assert!(parsed.is_err());
    assert_eq!(
        TransferError::Rejected(RejectReason::Declined).to_string(),
        "the peer declined the transfer"
    );
}
