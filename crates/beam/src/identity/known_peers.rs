//! The `known_peers` database: the trust root for receiving.

use std::fmt::Write as _;

use ed25519_dalek::VerifyingKey;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::keys::{KEY_TYPE, KeyError, decode_public_key, encode_public_key};
use super::{Fingerprint, ShortId};

/// Written at the top of a freshly created `known_peers` file.
pub const HEADER: &str = "# beam known_peers v1\n\
                          # format: <name>  ed25519 <base64 public key>  added=<RFC3339>\n";

/// The longest nickname we will store.
const MAX_NAME_LEN: usize = 32;

/// Why an operation on the peer database failed.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("peer not found: {0}")]
    NotFound(String),
    #[error("a peer with that name already exists: {0}")]
    Exists(String),
    #[error("that public key is already stored under another name: {0}")]
    KeyInUse(String),
    #[error("invalid peer name {0:?}: use 1-{MAX_NAME_LEN} characters from A-Z a-z 0-9 . _ -")]
    InvalidName(String),
    #[error(transparent)]
    Key(#[from] KeyError),
}

/// What is wrong with one line of `known_peers`.
#[derive(Debug, thiserror::Error)]
pub enum ParseErrorKind {
    #[error("expected: <name> {KEY_TYPE} <base64 public key>")]
    TooFewFields,
    #[error("unsupported key type {0:?}, want {KEY_TYPE:?}")]
    UnsupportedKeyType(String),
    #[error("unexpected token {0:?}: attributes must be key=value")]
    UnexpectedToken(String),
    #[error("invalid attribute key {0:?}")]
    InvalidAttrKey(String),
    #[error("invalid added= timestamp {0:?}")]
    InvalidTimestamp(String),
    #[error("duplicate peer name {name:?} (also on line {first_line})")]
    DuplicateName { name: String, first_line: usize },
    #[error("public key of {name:?} is already stored as {other:?}")]
    DuplicateKey { name: String, other: String },
    #[error(transparent)]
    Peer(#[from] PeerError),
}

/// A malformed `known_peers` line.
///
/// Parsing never skips a bad line: a peer database that silently drops entries
/// is a security problem, so the line number is always reported.
#[derive(Debug, thiserror::Error)]
#[error("known_peers line {line}: {kind}")]
pub struct ParseError {
    pub line: usize,
    #[source]
    pub kind: ParseErrorKind,
}

/// Reports whether a nickname may be stored in `known_peers`.
pub fn validate_name(name: &str) -> Result<(), PeerError> {
    let len = name.chars().count();
    let charset_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if len == 0 || len > MAX_NAME_LEN || !charset_ok {
        return Err(PeerError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// A `key=value` attribute on a peer line.
///
/// Attributes we do not recognise are preserved verbatim so that a newer beam
/// can add fields without this version silently destroying them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub key: String,
    pub value: String,
}

/// One entry in `known_peers`.
#[derive(Clone, Debug)]
pub struct Peer {
    pub name: String,
    pub public_key: VerifyingKey,
    pub added: Option<OffsetDateTime>,
    pub extra: Vec<Attr>,
}

impl Peer {
    /// Creates a peer entry stamped with the current time.
    pub fn new(name: impl Into<String>, public_key: VerifyingKey) -> Self {
        Self {
            name: name.into(),
            public_key,
            added: Some(now_utc_seconds()),
            extra: Vec::new(),
        }
    }

    /// The SHA-256 fingerprint of the peer's public key.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.public_key)
    }

    /// The peer's 9-digit lookup hint.
    pub fn short_id(&self) -> ShortId {
        self.fingerprint().short_id()
    }

    /// The `added=` date as `YYYY-MM-DD`, or `-` when it is absent.
    pub fn added_date(&self) -> String {
        match self.added {
            Some(t) => t.date().to_string(),
            None => "-".to_string(),
        }
    }

    /// The `added=` timestamp in RFC 3339, if present.
    pub fn added_rfc3339(&self) -> Option<String> {
        self.added.and_then(|t| t.format(&Rfc3339).ok())
    }
}

/// Either a peer entry or a verbatim comment/blank line.
#[derive(Clone, Debug)]
enum Line {
    Raw(String),
    Peer(Box<Peer>),
}

/// The parsed `known_peers` file.
///
/// Comments, blank lines and their order are preserved across edits.
#[derive(Clone, Debug, Default)]
pub struct KnownPeers {
    lines: Vec<Line>,
}

impl KnownPeers {
    /// An empty database carrying only the standard header.
    pub fn with_header() -> Self {
        Self::parse(HEADER).expect("the built-in header always parses")
    }

    /// Parses the contents of a `known_peers` file.
    pub fn parse(data: &str) -> Result<Self, ParseError> {
        let mut lines = Vec::new();
        let mut names: Vec<(String, usize)> = Vec::new();
        let mut keys: Vec<(String, String)> = Vec::new();

        for (index, raw) in data.lines().enumerate() {
            let number = index + 1;
            let text = raw.strip_suffix('\r').unwrap_or(raw);
            let trimmed = text.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                lines.push(Line::Raw(text.to_string()));
                continue;
            }

            let peer = parse_peer_line(text).map_err(|kind| ParseError { line: number, kind })?;

            let lowered = peer.name.to_ascii_lowercase();
            if let Some((_, first_line)) = names.iter().find(|(n, _)| *n == lowered) {
                return Err(ParseError {
                    line: number,
                    kind: ParseErrorKind::DuplicateName {
                        name: peer.name.clone(),
                        first_line: *first_line,
                    },
                });
            }
            names.push((lowered, number));

            let encoded = encode_public_key(&peer.public_key);
            if let Some((_, other)) = keys.iter().find(|(k, _)| *k == encoded) {
                return Err(ParseError {
                    line: number,
                    kind: ParseErrorKind::DuplicateKey {
                        name: peer.name.clone(),
                        other: other.clone(),
                    },
                });
            }
            keys.push((encoded, peer.name.clone()));

            lines.push(Line::Peer(Box::new(peer)));
        }
        Ok(Self { lines })
    }

    /// Renders the file. Peer names are padded so the columns line up.
    pub fn render(&self) -> String {
        let width = self
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Peer(p) => Some(p.name.chars().count()),
                Line::Raw(_) => None,
            })
            .max()
            .unwrap_or(0);

        let mut out = String::new();
        for line in &self.lines {
            match line {
                Line::Raw(text) => {
                    out.push_str(text);
                    out.push('\n');
                }
                Line::Peer(peer) => {
                    let _ = write!(
                        out,
                        "{:<width$}  {KEY_TYPE} {}",
                        peer.name,
                        encode_public_key(&peer.public_key),
                        width = width
                    );
                    if let Some(added) = peer.added_rfc3339() {
                        let _ = write!(out, "  added={added}");
                    }
                    for attr in &peer.extra {
                        let _ = write!(out, " {}={}", attr.key, attr.value);
                    }
                    out.push('\n');
                }
            }
        }
        out
    }

    /// The stored peers, in file order.
    pub fn peers(&self) -> Vec<&Peer> {
        self.lines
            .iter()
            .filter_map(|l| match l {
                Line::Peer(p) => Some(p.as_ref()),
                Line::Raw(_) => None,
            })
            .collect()
    }

    /// The number of stored peers.
    pub fn len(&self) -> usize {
        self.peers().len()
    }

    /// Whether the database holds no peers.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Finds a peer by nickname, case-insensitively.
    pub fn lookup(&self, name: &str) -> Option<&Peer> {
        self.index_of(name).map(|i| match &self.lines[i] {
            Line::Peer(p) => p.as_ref(),
            Line::Raw(_) => unreachable!("index_of only returns peer lines"),
        })
    }

    /// Finds a peer by public key.
    pub fn lookup_key(&self, key: &VerifyingKey) -> Option<&Peer> {
        self.peers().into_iter().find(|p| p.public_key == *key)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.lines.iter().position(|l| match l {
            Line::Peer(p) => p.name.eq_ignore_ascii_case(name),
            Line::Raw(_) => false,
        })
    }

    /// Appends a peer.
    ///
    /// Refuses duplicate names (case-insensitively) and refuses to store one
    /// public key under two different names.
    pub fn add(&mut self, peer: Peer) -> Result<(), PeerError> {
        validate_name(&peer.name)?;
        if self.lookup(&peer.name).is_some() {
            return Err(PeerError::Exists(peer.name));
        }
        if let Some(other) = self.lookup_key(&peer.public_key) {
            return Err(PeerError::KeyInUse(other.name.clone()));
        }
        let mut peer = peer;
        if peer.added.is_none() {
            peer.added = Some(now_utc_seconds());
        }
        self.lines.push(Line::Peer(Box::new(peer)));
        Ok(())
    }

    /// Changes a peer's nickname, keeping its key and its place in the file.
    pub fn rename(&mut self, old_name: &str, new_name: &str) -> Result<(), PeerError> {
        validate_name(new_name)?;
        let index = self
            .index_of(old_name)
            .ok_or_else(|| PeerError::NotFound(old_name.to_string()))?;
        if !old_name.eq_ignore_ascii_case(new_name) && self.lookup(new_name).is_some() {
            return Err(PeerError::Exists(new_name.to_string()));
        }
        match &mut self.lines[index] {
            Line::Peer(p) => p.name = new_name.to_string(),
            Line::Raw(_) => unreachable!("index_of only returns peer lines"),
        }
        Ok(())
    }

    /// Deletes a peer by nickname.
    pub fn remove(&mut self, name: &str) -> Result<(), PeerError> {
        let index = self
            .index_of(name)
            .ok_or_else(|| PeerError::NotFound(name.to_string()))?;
        self.lines.remove(index);
        Ok(())
    }
}

fn parse_peer_line(text: &str) -> Result<Peer, ParseErrorKind> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    if fields.len() < 3 {
        return Err(ParseErrorKind::TooFewFields);
    }
    validate_name(fields[0]).map_err(ParseErrorKind::Peer)?;
    if fields[1] != KEY_TYPE {
        return Err(ParseErrorKind::UnsupportedKeyType(fields[1].to_string()));
    }
    let public_key =
        decode_public_key(fields[2]).map_err(|e| ParseErrorKind::Peer(PeerError::Key(e)))?;

    let mut peer = Peer {
        name: fields[0].to_string(),
        public_key,
        added: None,
        extra: Vec::new(),
    };

    for token in &fields[3..] {
        let Some((key, value)) = token.split_once('=') else {
            return Err(ParseErrorKind::UnexpectedToken(token.to_string()));
        };
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        {
            return Err(ParseErrorKind::InvalidAttrKey(key.to_string()));
        }
        if key == "added" {
            let parsed = OffsetDateTime::parse(value, &Rfc3339)
                .map_err(|_| ParseErrorKind::InvalidTimestamp(value.to_string()))?;
            peer.added = Some(parsed);
            continue;
        }
        peer.extra.push(Attr {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    Ok(peer)
}

/// The current UTC time truncated to whole seconds, so rendered timestamps
/// stay short and round-trip exactly.
fn now_utc_seconds() -> OffsetDateTime {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero is a valid nanosecond")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vectors::{VECTORS, verifying_key};

    fn line_for(vector: usize, name: &str) -> String {
        format!("{name} {KEY_TYPE} {}\n", VECTORS[vector].public_b64)
    }

    #[test]
    fn parses_a_well_formed_file() {
        let input = format!(
            "# beam known_peers v1\n\
             \n\
             alice  {KEY_TYPE} {}  added=2026-01-02T03:04:05Z\n\
             bob\t{KEY_TYPE}\t{}\tadded=2026-02-03T04:05:06Z note=work-laptop\n",
            VECTORS[0].public_b64, VECTORS[1].public_b64
        );

        let known = KnownPeers::parse(&input).expect("parse");
        let peers = known.peers();
        assert_eq!(peers.len(), 2);

        assert_eq!(peers[0].name, "alice");
        assert_eq!(peers[0].fingerprint().hex(), VECTORS[0].fingerprint_hex);
        assert_eq!(
            peers[0].added_rfc3339().as_deref(),
            Some("2026-01-02T03:04:05Z")
        );

        assert_eq!(
            peers[1].extra,
            vec![Attr {
                key: "note".to_string(),
                value: "work-laptop".to_string()
            }],
            "an unknown attribute was not preserved"
        );
    }

    #[test]
    fn an_empty_file_has_no_peers() {
        assert_eq!(KnownPeers::parse("").expect("parse").len(), 0);
    }

    #[test]
    fn rejects_malformed_lines_by_line_number() {
        let good = line_for(0, "alice");
        let stem = good.trim_end();
        let cases = [
            ("too few fields", format!("alice {KEY_TYPE}\n")),
            (
                "wrong key type",
                format!("alice ssh-rsa {}\n", VECTORS[0].public_b64),
            ),
            ("bad base64", format!("alice {KEY_TYPE} not!base64\n")),
            ("short key", format!("alice {KEY_TYPE} AAAA\n")),
            (
                "invalid name",
                format!("al!ice {KEY_TYPE} {}\n", VECTORS[0].public_b64),
            ),
            ("bare token", format!("{stem} trailing\n")),
            ("bad attribute key", format!("{stem} bad+key=1\n")),
            ("bad timestamp", format!("{stem} added=yesterday\n")),
            ("duplicate name", format!("{good}{}", line_for(1, "alice"))),
            ("duplicate key", format!("{good}{}", line_for(0, "carol"))),
            (
                "name differing only in case",
                format!("{good}{}", line_for(1, "ALICE")),
            ),
        ];
        for (name, input) in cases {
            let err = KnownPeers::parse(&input)
                .err()
                .unwrap_or_else(|| panic!("{name}: parsed without error"));
            assert!(err.line >= 1, "{name}: no line number");
        }
    }

    #[test]
    fn rendering_preserves_comments_and_attributes() {
        let input = format!(
            "# beam known_peers v1\n\
             # hand-written note\n\
             \n\
             alice {KEY_TYPE} {} added=2026-01-02T03:04:05Z note=home\n",
            VECTORS[0].public_b64
        );

        let rendered = KnownPeers::parse(&input).expect("parse").render();
        for expected in [
            "# beam known_peers v1",
            "# hand-written note",
            "note=home",
            "added=2026-01-02T03:04:05Z",
        ] {
            assert!(
                rendered.contains(expected),
                "lost {expected:?}:\n{rendered}"
            );
        }

        // Rendering must be stable: parse(render(x)) == render(x).
        let again = KnownPeers::parse(&rendered).expect("reparse").render();
        assert_eq!(again, rendered, "render is not idempotent");
    }

    #[test]
    fn add_stamps_a_date_and_refuses_duplicates() {
        let mut known = KnownPeers::with_header();
        known
            .add(Peer::new("alice", verifying_key("alpha")))
            .expect("add");
        assert!(known.lookup("alice").expect("stored").added.is_some());

        let err = known
            .add(Peer::new("ALICE", verifying_key("bravo")))
            .unwrap_err();
        assert!(matches!(err, PeerError::Exists(_)), "{err}");

        let err = known
            .add(Peer::new("alice2", verifying_key("alpha")))
            .unwrap_err();
        assert!(matches!(err, PeerError::KeyInUse(_)), "{err}");

        let err = known
            .add(Peer::new("bad name", verifying_key("bravo")))
            .unwrap_err();
        assert!(matches!(err, PeerError::InvalidName(_)), "{err}");
    }

    #[test]
    fn lookup_is_case_insensitive_and_lookup_key_matches_by_key() {
        let known = KnownPeers::parse(&line_for(0, "alice")).expect("parse");
        assert!(known.lookup("ALICE").is_some());
        assert!(known.lookup("nobody").is_none());
        assert_eq!(
            known.lookup_key(&verifying_key("alpha")).map(|p| &p.name),
            Some(&"alice".to_string())
        );
        assert!(known.lookup_key(&verifying_key("bravo")).is_none());
    }

    #[test]
    fn rename_keeps_the_key_the_comments_and_the_order() {
        let input = format!(
            "# note\nalice {KEY_TYPE} {} note=home\nbob {KEY_TYPE} {}\n",
            VECTORS[0].public_b64, VECTORS[1].public_b64
        );
        let mut known = KnownPeers::parse(&input).expect("parse");

        known.rename("alice", "ali").expect("rename");
        assert!(known.lookup("ali").is_some());
        assert!(known.lookup("alice").is_none());
        assert_eq!(known.peers()[0].name, "ali", "rename changed the order");

        let rendered = known.render();
        assert!(rendered.contains("note=home") && rendered.contains("# note"));

        assert!(matches!(
            known.rename("ali", "bob").unwrap_err(),
            PeerError::Exists(_)
        ));
        assert!(matches!(
            known.rename("nobody", "x").unwrap_err(),
            PeerError::NotFound(_)
        ));
        assert!(matches!(
            known.rename("ali", "not a name").unwrap_err(),
            PeerError::InvalidName(_)
        ));
        // Changing only the capitalisation of a peer's own name is allowed.
        known.rename("ali", "Ali").expect("recapitalise");
    }

    #[test]
    fn remove_is_case_insensitive_and_keeps_comments() {
        let input = format!(
            "# note\nalice {KEY_TYPE} {}\nbob {KEY_TYPE} {}\n",
            VECTORS[0].public_b64, VECTORS[1].public_b64
        );
        let mut known = KnownPeers::parse(&input).expect("parse");

        known.remove("ALICE").expect("remove");
        assert_eq!(known.len(), 1);
        assert!(known.lookup("alice").is_none());
        assert!(known.render().contains("# note"));
        assert!(matches!(
            known.remove("alice").unwrap_err(),
            PeerError::NotFound(_)
        ));
    }

    #[test]
    fn name_rules() {
        for name in [
            "a",
            "alice",
            "Alice-2",
            "my.laptop",
            "under_score",
            &"x".repeat(32),
        ] {
            assert!(validate_name(name).is_ok(), "rejected {name:?}");
        }
        for name in [
            "",
            " ",
            "with space",
            "emoji\u{2728}",
            "slash/name",
            "hash#name",
            &"x".repeat(33),
        ] {
            assert!(validate_name(name).is_err(), "accepted {name:?}");
        }
    }
}
