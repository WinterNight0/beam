package identity

import (
	"bufio"
	"bytes"
	"crypto/ed25519"
	"crypto/subtle"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// Header is written at the top of a freshly created known_peers file.
const Header = "# beam known_peers v1\n" +
	"# format: <name>  ed25519 <base64 public key>  added=<RFC3339>\n"

// Sentinel errors for peer bookkeeping.
var (
	ErrPeerNotFound = errors.New("peer not found")
	ErrPeerExists   = errors.New("a peer with that name already exists")
	ErrKeyInUse     = errors.New("that public key is already stored under another name")
	ErrInvalidName  = errors.New("invalid peer name")
)

// nameRE constrains nicknames to characters that are unambiguous in a terminal
// and safe as a whitespace-separated token in known_peers.
var nameRE = regexp.MustCompile(`^[A-Za-z0-9._-]{1,32}$`)

// attrKeyRE constrains attribute keys to the same conservative alphabet.
var attrKeyRE = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)

// ValidateName reports whether a nickname may be stored in known_peers.
func ValidateName(name string) error {
	if !nameRE.MatchString(name) {
		return fmt.Errorf("%w %q: use 1-32 characters from A-Z a-z 0-9 . _ -", ErrInvalidName, name)
	}
	return nil
}

// Attr is a key=value attribute on a peer line. Attributes we do not recognise
// are preserved verbatim so that a newer beam can add fields without this
// version silently destroying them.
type Attr struct {
	Key   string
	Value string
}

// Peer is one entry in known_peers.
type Peer struct {
	Name      string
	PublicKey ed25519.PublicKey
	Added     time.Time
	Extra     []Attr
}

// Fingerprint returns the SHA-256 fingerprint of the peer's public key.
func (p Peer) Fingerprint() Fingerprint { return FingerprintOf(p.PublicKey) }

// ShortID returns the peer's 9-digit lookup hint.
func (p Peer) ShortID() ShortID { return p.Fingerprint().ShortID() }

// ParseError reports a malformed known_peers line. Parsing never skips a bad
// line: a peer database that silently drops entries is a security problem.
type ParseError struct {
	Line int
	Err  error
}

func (e *ParseError) Error() string { return fmt.Sprintf("known_peers line %d: %v", e.Line, e.Err) }

func (e *ParseError) Unwrap() error { return e.Err }

// line is either a peer entry or a verbatim comment/blank line.
type line struct {
	raw  string
	peer *Peer
}

// KnownPeers is the parsed known_peers file. Comments, blank lines and their
// order are preserved across edits.
type KnownPeers struct {
	lines []line
}

// ParseKnownPeers parses the contents of a known_peers file.
func ParseKnownPeers(data []byte) (*KnownPeers, error) {
	kp := &KnownPeers{}
	sc := bufio.NewScanner(bytes.NewReader(data))
	sc.Buffer(make([]byte, 0, 64*1024), 1024*1024)

	byName := map[string]int{}
	byKey := map[string]string{}

	for n := 1; sc.Scan(); n++ {
		text := strings.TrimRight(sc.Text(), "\r")
		trimmed := strings.TrimSpace(text)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			kp.lines = append(kp.lines, line{raw: text})
			continue
		}
		peer, err := parsePeerLine(text)
		if err != nil {
			return nil, &ParseError{Line: n, Err: err}
		}
		key := strings.ToLower(peer.Name)
		if prev, dup := byName[key]; dup {
			return nil, &ParseError{Line: n, Err: fmt.Errorf("duplicate peer name %q (also on line %d)", peer.Name, prev)}
		}
		byName[key] = n
		enc := EncodePublicKey(peer.PublicKey)
		if other, dup := byKey[enc]; dup {
			return nil, &ParseError{Line: n, Err: fmt.Errorf("public key of %q is already stored as %q", peer.Name, other)}
		}
		byKey[enc] = peer.Name
		kp.lines = append(kp.lines, line{peer: peer})
	}
	if err := sc.Err(); err != nil {
		return nil, fmt.Errorf("read known_peers: %w", err)
	}
	return kp, nil
}

func parsePeerLine(text string) (*Peer, error) {
	fields := strings.Fields(text)
	if len(fields) < 3 {
		return nil, errors.New("expected: <name> ed25519 <base64 public key>")
	}
	if err := ValidateName(fields[0]); err != nil {
		return nil, err
	}
	if fields[1] != KeyType {
		return nil, fmt.Errorf("unsupported key type %q, want %q", fields[1], KeyType)
	}
	pub, err := DecodePublicKey(fields[2])
	if err != nil {
		return nil, err
	}
	peer := &Peer{Name: fields[0], PublicKey: pub}
	for _, tok := range fields[3:] {
		k, v, ok := strings.Cut(tok, "=")
		if !ok || k == "" {
			return nil, fmt.Errorf("unexpected token %q: attributes must be key=value", tok)
		}
		if !attrKeyRE.MatchString(k) {
			return nil, fmt.Errorf("invalid attribute key %q", k)
		}
		if k == "added" {
			t, err := time.Parse(time.RFC3339, v)
			if err != nil {
				return nil, fmt.Errorf("invalid added= timestamp %q: %w", v, err)
			}
			peer.Added = t
			continue
		}
		peer.Extra = append(peer.Extra, Attr{Key: k, Value: v})
	}
	return peer, nil
}

// Bytes renders the file. Peer names are padded so the columns line up.
func (kp *KnownPeers) Bytes() []byte {
	width := 0
	for _, l := range kp.lines {
		if l.peer != nil && len(l.peer.Name) > width {
			width = len(l.peer.Name)
		}
	}
	var buf bytes.Buffer
	for _, l := range kp.lines {
		if l.peer == nil {
			buf.WriteString(l.raw)
			buf.WriteByte('\n')
			continue
		}
		fmt.Fprintf(&buf, "%-*s  %s %s", width, l.peer.Name, KeyType, EncodePublicKey(l.peer.PublicKey))
		if !l.peer.Added.IsZero() {
			buf.WriteString("  added=" + l.peer.Added.UTC().Format(time.RFC3339))
		}
		for _, a := range l.peer.Extra {
			buf.WriteString(" " + a.Key + "=" + a.Value)
		}
		buf.WriteByte('\n')
	}
	return buf.Bytes()
}

// Peers returns the stored peers in file order.
func (kp *KnownPeers) Peers() []Peer {
	out := make([]Peer, 0, len(kp.lines))
	for _, l := range kp.lines {
		if l.peer != nil {
			out = append(out, *l.peer)
		}
	}
	return out
}

// Len returns the number of stored peers.
func (kp *KnownPeers) Len() int { return len(kp.Peers()) }

// Lookup finds a peer by nickname, case-insensitively.
func (kp *KnownPeers) Lookup(name string) (Peer, bool) {
	if l := kp.find(name); l != nil {
		return *l.peer, true
	}
	return Peer{}, false
}

// LookupKey finds a peer by public key.
func (kp *KnownPeers) LookupKey(pub ed25519.PublicKey) (Peer, bool) {
	for _, l := range kp.lines {
		if l.peer != nil && subtle.ConstantTimeCompare(l.peer.PublicKey, pub) == 1 {
			return *l.peer, true
		}
	}
	return Peer{}, false
}

func (kp *KnownPeers) find(name string) *line {
	for i := range kp.lines {
		if kp.lines[i].peer != nil && strings.EqualFold(kp.lines[i].peer.Name, name) {
			return &kp.lines[i]
		}
	}
	return nil
}

// Add appends a peer. It refuses duplicate names (case-insensitively) and
// refuses to store one public key under two different names.
func (kp *KnownPeers) Add(p Peer) error {
	if err := ValidateName(p.Name); err != nil {
		return err
	}
	if len(p.PublicKey) != ed25519.PublicKeySize {
		return fmt.Errorf("public key must be %d bytes, got %d", ed25519.PublicKeySize, len(p.PublicKey))
	}
	if _, exists := kp.Lookup(p.Name); exists {
		return fmt.Errorf("%w: %s", ErrPeerExists, p.Name)
	}
	if other, exists := kp.LookupKey(p.PublicKey); exists {
		return fmt.Errorf("%w: %s", ErrKeyInUse, other.Name)
	}
	if p.Added.IsZero() {
		p.Added = time.Now().UTC()
	}
	kp.lines = append(kp.lines, line{peer: &p})
	return nil
}

// Rename changes a peer's nickname, keeping its key and its place in the file.
func (kp *KnownPeers) Rename(oldName, newName string) error {
	if err := ValidateName(newName); err != nil {
		return err
	}
	l := kp.find(oldName)
	if l == nil {
		return fmt.Errorf("%w: %s", ErrPeerNotFound, oldName)
	}
	if !strings.EqualFold(oldName, newName) {
		if _, exists := kp.Lookup(newName); exists {
			return fmt.Errorf("%w: %s", ErrPeerExists, newName)
		}
	}
	l.peer.Name = newName
	return nil
}

// Remove deletes a peer by nickname.
func (kp *KnownPeers) Remove(name string) error {
	for i := range kp.lines {
		if kp.lines[i].peer != nil && strings.EqualFold(kp.lines[i].peer.Name, name) {
			kp.lines = append(kp.lines[:i], kp.lines[i+1:]...)
			return nil
		}
	}
	return fmt.Errorf("%w: %s", ErrPeerNotFound, name)
}
