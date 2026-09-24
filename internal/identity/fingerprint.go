package identity

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"strings"
)

// FingerprintSize is the length in bytes of a peer fingerprint (SHA-256).
const FingerprintSize = sha256.Size

// shortIDModulus bounds a short ID to exactly 9 decimal digits.
const shortIDModulus = 1_000_000_000

// Fingerprint is the SHA-256 digest of a raw 32-byte Ed25519 public key.
// It is the canonical, full-strength identifier for a device: the signaling
// server routes by it, and users compare it out of band.
type Fingerprint [FingerprintSize]byte

// FingerprintOf derives the fingerprint of a public key.
func FingerprintOf(pub ed25519.PublicKey) Fingerprint {
	return Fingerprint(sha256.Sum256(pub))
}

// Hex returns the canonical lowercase hex form (64 characters).
func (f Fingerprint) Hex() string { return hex.EncodeToString(f[:]) }

// String returns the display form, e.g. "SHA256:1a2b...".
func (f Fingerprint) String() string { return "SHA256:" + f.Hex() }

// Short returns an abbreviated display form for dense output such as tables.
// It is never safe to compare identities by the short form.
func (f Fingerprint) Short() string { return "SHA256:" + f.Hex()[:16] + "..." }

// ParseFingerprint accepts the canonical hex form, with or without the
// "SHA256:" prefix, ignoring case and any colons used as separators.
func ParseFingerprint(s string) (Fingerprint, error) {
	var f Fingerprint
	t := strings.TrimSpace(s)
	if strings.HasPrefix(strings.ToUpper(t), "SHA256:") {
		t = t[len("SHA256:"):]
	}
	t = strings.ToLower(strings.ReplaceAll(t, ":", ""))
	if len(t) != hex.EncodedLen(FingerprintSize) {
		return f, fmt.Errorf("fingerprint must be %d hex characters, got %d", hex.EncodedLen(FingerprintSize), len(t))
	}
	raw, err := hex.DecodeString(t)
	if err != nil {
		return f, fmt.Errorf("fingerprint is not valid hex: %w", err)
	}
	copy(f[:], raw)
	return f, nil
}

// ShortID is a 9-digit lookup hint derived from a fingerprint. It exists only
// so a human can read an identifier aloud for the very first pairing; it is a
// routing hint, NOT a security guarantee. Security comes from the PAKE during
// pairing and from the stored public key afterwards.
type ShortID uint32

// ShortID derives the 9-digit short ID from the fingerprint.
func (f Fingerprint) ShortID() ShortID {
	return ShortID(binary.BigEndian.Uint64(f[:8]) % shortIDModulus)
}

// String returns the zero-padded 9-digit form, e.g. "004815162".
func (s ShortID) String() string { return fmt.Sprintf("%09d", uint32(s)) }

// Display groups the digits for readability, e.g. "004 815 162".
func (s ShortID) Display() string {
	d := s.String()
	return d[0:3] + " " + d[3:6] + " " + d[6:9]
}

// ParseShortID accepts 9 digits with optional spaces or dashes between them.
func ParseShortID(s string) (ShortID, error) {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= '0' && r <= '9':
			b.WriteRune(r)
		case r == ' ' || r == '-' || r == '\t':
			// separators are cosmetic
		default:
			return 0, fmt.Errorf("short ID contains invalid character %q", r)
		}
	}
	digits := b.String()
	if len(digits) != 9 {
		return 0, fmt.Errorf("short ID must have 9 digits, got %d", len(digits))
	}
	var v uint64
	for _, r := range digits {
		v = v*10 + uint64(r-'0')
	}
	return ShortID(v), nil
}
