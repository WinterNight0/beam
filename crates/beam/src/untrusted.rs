//! Making text from the other side safe to put on a terminal.
//!
//! A file name, a peer hint, a cancel reason, an error message quoting what
//! a peer sent, the network service's error text: all of it is chosen by
//! someone else, and a terminal interprets some characters as commands. An
//! ANSI sequence can recolour the screen, move the cursor, rewrite a line the
//! user already read, or set the window title; a carriage return can print
//! `SHA256:<attacker>` over `SHA256:<real>`; a right-to-left override makes
//! `invoice\u{202E}fdp.exe` read as `invoiceexe.pdf`. See ADR-0034.
//!
//! What this module does, in order:
//!
//! 1. **Escape sequences are removed whole** — CSI (`ESC [ … final`), OSC
//!    (`ESC ] … BEL|ST`), the DCS/SOS/PM/APC strings, their 8-bit C1 forms, and
//!    two-byte `ESC x` — so no `[31m` debris is left behind.
//! 2. **Every other control character is removed**: C0 (including `\r`, BEL,
//!    backspace), DEL and C1. A tab becomes a space.
//! 3. **Invisible direction and joining characters are made visible** as
//!    `<U+XXXX>`: the bidi overrides and isolates, LRM/RLM/ALM, and the
//!    zero-width space, joiners and friends. Nothing is silently dropped, so
//!    the person sees that the text contains something odd. (File names with
//!    bidi overrides or isolates never get this far: they are refused,
//!    `transfer::paths`. Zero-width characters are legitimate in names — ZWSP
//!    in Thai text copied from the web, ZWJ inside emoji — so they are allowed
//!    and shown.)
//! 4. **The length is capped.** Names are cut in the middle so the extension
//!    stays visible: a long name must not be able to push `.exe` off the end.
//!
//! Everything else — Thai vowels and tone marks, emoji, accents — passes
//! through unchanged.

/// Longest name shown, in characters.
pub const NAME_MAX: usize = 60;

/// Longest message or reason shown, in characters.
pub const TEXT_MAX: usize = 300;

/// The mark used where text was cut.
const ELLIPSIS: char = '…';

/// Text from the other side, made safe to print on one line.
pub fn text(raw: &str) -> String {
    cap_end(&clean(raw, false), TEXT_MAX)
}

/// Like [`text`], but keeps line breaks — for our own multi-line error
/// messages, which may quote something from the other side.
pub fn lines(raw: &str) -> String {
    cap_end(&clean(raw, true), TEXT_MAX * 4)
}

/// A file name or a nickname from the other side, made safe to print, cut in
/// the middle if it is long so that its extension stays visible.
pub fn name(raw: &str) -> String {
    cap_middle(&clean(raw, false), NAME_MAX)
}

/// Whether `c` is a bidi override or isolate. File names containing one are
/// refused outright (`transfer::paths`).
pub fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Invisible characters that are shown as `<U+XXXX>` rather than printed.
fn is_invisible(c: char) -> bool {
    is_bidi_control(c)
        || matches!(
            c,
            '\u{200B}'..='\u{200F}' // ZWSP, ZWNJ, ZWJ, LRM, RLM
            | '\u{061C}'            // ARABIC LETTER MARK
            | '\u{2060}'..='\u{2064}' // WORD JOINER, invisible operators
            | '\u{FEFF}'            // ZERO WIDTH NO-BREAK SPACE / BOM
            | '\u{180E}'            // MONGOLIAN VOWEL SEPARATOR
        )
}

/// Steps 1–3.
fn clean(raw: &str, keep_newlines: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1B}' => skip_escape(&mut chars),
            // 8-bit CSI, OSC, and the string introducers.
            '\u{9B}' => skip_csi(&mut chars),
            '\u{9D}' | '\u{90}' | '\u{98}' | '\u{9E}' | '\u{9F}' => skip_string(&mut chars),
            '\n' if keep_newlines => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c if is_invisible(c) => out.push_str(&format!("<U+{:04X}>", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => skip_csi(chars),
        Some(']' | 'P' | 'X' | '^' | '_') => skip_string(chars),
        // `ESC x`: one more character, e.g. ESC c (reset) or ESC 7.
        Some(_) | None => {}
    }
}

/// CSI: parameter and intermediate bytes, then one final byte in `@`..`~`.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7E}').contains(&c) {
            return;
        }
        // Anything that cannot be part of a CSI ends it too, so a malformed
        // sequence cannot swallow the rest of the text.
        if !('\u{20}'..='\u{3F}').contains(&c) {
            return;
        }
    }
}

/// OSC/DCS/SOS/PM/APC: everything up to BEL, ST (`ESC \`), or 8-bit ST.
fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        match c {
            '\u{07}' | '\u{9C}' => return,
            '\u{1B}' => {
                if chars.peek() == Some(&'\\') {
                    chars.next();
                }
                return;
            }
            _ => {}
        }
    }
}

fn cap_end(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push(ELLIPSIS);
    out
}

/// Cuts the middle out of a long name, keeping its end — at least the last
/// twelve characters, and always the whole final extension — so `.exe` cannot
/// be pushed out of sight.
fn cap_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let extension = chars
        .iter()
        .rposition(|c| *c == '.')
        .map(|dot| chars.len() - dot)
        .unwrap_or(0);
    let tail = extension.max(12).min(max / 2);
    let head = max - tail - 1;
    let mut out: String = chars[..head].iter().collect();
    out.push(ELLIPSIS);
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        for s in [
            "report.pdf",
            "hello world",
            "résumé (final).docx",
            "a-b_c.1",
        ] {
            assert_eq!(text(s), s);
            assert_eq!(name(s), s);
        }
    }

    /// M6 answer 1: Thai with its vowels and tone marks passes unchanged.
    #[test]
    fn thai_text_passes_through_unchanged() {
        let thai = "รายงานประจำปี_ฉบับที่๒_ผู้อำนวยการ.pdf";
        assert_eq!(name(thai), thai);
        assert_eq!(text(thai), thai);
        // Combining marks are not "control" or "invisible": nothing to show.
        assert!(!name(thai).contains("<U+"));
    }

    /// M6 answer 1: ZWJ inside an emoji is allowed, and shown safely.
    #[test]
    fn an_emoji_with_zwj_is_shown_with_the_joiner_visible() {
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} holiday.jpg";
        assert_eq!(
            name(family),
            "\u{1F468}<U+200D>\u{1F469}<U+200D>\u{1F467} holiday.jpg"
        );
    }

    #[test]
    fn zero_width_space_and_marks_are_made_visible() {
        assert_eq!(name("ข่าว\u{200B}ดี.txt"), "ข่าว<U+200B>ดี.txt");
        assert_eq!(text("a\u{200E}b\u{200F}c"), "a<U+200E>b<U+200F>c");
        assert_eq!(text("\u{FEFF}x"), "<U+FEFF>x");
    }

    #[test]
    fn bidi_overrides_and_isolates_are_made_visible() {
        assert_eq!(name("invoice\u{202E}fdp.exe"), "invoice<U+202E>fdp.exe");
        assert_eq!(text("\u{2066}x\u{2069}"), "<U+2066>x<U+2069>");
        for c in ['\u{202A}', '\u{202B}', '\u{202C}', '\u{202D}', '\u{202E}'] {
            assert!(is_bidi_control(c));
        }
        for c in ['\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}'] {
            assert!(is_bidi_control(c));
        }
        // Zero-width characters are made visible but are not bidi controls.
        assert!(!is_bidi_control('\u{200B}'));
        assert!(!is_bidi_control('\u{200D}'));
    }

    #[test]
    fn ansi_sequences_are_removed_whole() {
        let cases = [
            ("\u{1B}[31mred\u{1B}[0m", "red"),
            ("\u{1B}[2J\u{1B}[Hcleared", "cleared"),
            ("\u{1B}[1;31;47mloud", "loud"),
            ("\u{1B}]0;pwned\u{07}title", "title"),
            (
                "\u{1B}]8;;https://evil\u{1B}\\link\u{1B}]8;;\u{1B}\\",
                "link",
            ),
            ("\u{1B}Pdevice control\u{1B}\\after", "after"),
            ("\u{9B}31mc1\u{9B}0m", "c1"),
            ("\u{9D}0;t\u{9C}x", "x"),
            ("\u{1B}cfull reset", "full reset"),
            ("\u{1B}", ""),
            ("unterminated\u{1B}[31", "unterminated"),
        ];
        for (raw, want) in cases {
            assert_eq!(text(raw), want, "{raw:?}");
            assert!(!text(raw).contains('\u{1B}'));
        }
    }

    #[test]
    fn a_carriage_return_cannot_overwrite_what_was_shown() {
        // On a terminal this prints the fake fingerprint over the real one.
        let spoof = "SHA256:aaaa\rSHA256:bbbb";
        assert_eq!(text(spoof), "SHA256:aaaaSHA256:bbbb");
    }

    #[test]
    fn other_control_characters_are_removed() {
        assert_eq!(text("a\u{07}b\u{08}c\u{7F}d\u{85}e\u{0}f"), "abcdef");
        assert_eq!(text("tab\there"), "tab here");
        assert_eq!(text("two\nlines"), "twolines");
        assert_eq!(lines("two\nlines\u{1B}[2J"), "two\nlines");
    }

    #[test]
    fn long_text_is_capped() {
        let long = "x".repeat(1000);
        let shown = text(&long);
        assert_eq!(shown.chars().count(), TEXT_MAX);
        assert!(shown.ends_with(ELLIPSIS));
    }

    /// M6 answer 5: a long name is cut in the middle, and the extension
    /// survives. Cutting at the end would hide exactly the part that matters.
    #[test]
    fn a_long_name_cannot_push_exe_out_of_sight() {
        let evil = format!("Quarterly_report_{}.pdf.exe", "_final".repeat(20));
        let naive: String = evil.chars().take(NAME_MAX).collect();
        assert!(
            !naive.contains(".exe"),
            "the test needs a name that hides .exe"
        );

        let shown = name(&evil);
        assert!(shown.chars().count() <= NAME_MAX, "{shown}");
        assert!(shown.ends_with(".pdf.exe"), "{shown}");
        assert!(shown.starts_with("Quarterly_report"), "{shown}");
        assert!(shown.contains(ELLIPSIS), "{shown}");
    }

    #[test]
    fn a_long_extension_is_kept_whole() {
        let long_ext = format!("{}.{}", "a".repeat(80), "exe_really_long_ext");
        let shown = name(&long_ext);
        assert!(shown.ends_with(".exe_really_long_ext"), "{shown}");
        assert!(shown.chars().count() <= NAME_MAX);
    }

    #[test]
    fn short_names_are_left_alone() {
        let exact = "y".repeat(NAME_MAX);
        assert_eq!(name(&exact), exact);
    }

    #[test]
    fn cleaning_twice_changes_nothing_more() {
        for raw in [
            "\u{1B}[31mx\u{202E}y\u{200B}z",
            "ok.txt",
            &format!("{}.exe", "n".repeat(100)),
        ] {
            assert_eq!(name(&name(raw)), name(raw));
            assert_eq!(text(&text(raw)), text(raw));
        }
    }
}
