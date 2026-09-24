package identity

import (
	"crypto/ed25519"
	"crypto/rand"
	"crypto/x509"
	"encoding/base64"
	"encoding/pem"
	"errors"
	"fmt"
	"strings"
)

const (
	// KeyType is the literal written into known_peers and id_ed25519.pub.
	KeyType = "ed25519"

	privatePEMType = "BEAM PRIVATE KEY"
)

// ErrNoIdentity is returned when this device has no keypair yet.
var ErrNoIdentity = errors.New("no identity found; run `beam init` first")

// Identity is this device's Ed25519 keypair. The private half never leaves the
// device and is never sent to the signaling server.
type Identity struct {
	Private ed25519.PrivateKey
	Public  ed25519.PublicKey
	Comment string
}

// Generate creates a fresh keypair using crypto/rand.
func Generate(comment string) (*Identity, error) {
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, fmt.Errorf("generate ed25519 key: %w", err)
	}
	return &Identity{Private: priv, Public: pub, Comment: sanitizeComment(comment)}, nil
}

// Fingerprint returns the SHA-256 fingerprint of the public key.
func (i *Identity) Fingerprint() Fingerprint { return FingerprintOf(i.Public) }

// ShortID returns the 9-digit pairing lookup hint.
func (i *Identity) ShortID() ShortID { return i.Fingerprint().ShortID() }

// MarshalPrivateKey encodes a private key as PEM-wrapped PKCS#8 DER.
func MarshalPrivateKey(priv ed25519.PrivateKey) ([]byte, error) {
	der, err := x509.MarshalPKCS8PrivateKey(priv)
	if err != nil {
		return nil, fmt.Errorf("marshal private key: %w", err)
	}
	return pem.EncodeToMemory(&pem.Block{Type: privatePEMType, Bytes: der}), nil
}

// UnmarshalPrivateKey decodes a PEM-wrapped PKCS#8 Ed25519 private key.
func UnmarshalPrivateKey(data []byte) (ed25519.PrivateKey, error) {
	block, _ := pem.Decode(data)
	if block == nil {
		return nil, errors.New("private key file is not valid PEM")
	}
	if block.Type != privatePEMType {
		return nil, fmt.Errorf("unexpected PEM block %q, want %q", block.Type, privatePEMType)
	}
	key, err := x509.ParsePKCS8PrivateKey(block.Bytes)
	if err != nil {
		return nil, fmt.Errorf("parse private key: %w", err)
	}
	priv, ok := key.(ed25519.PrivateKey)
	if !ok {
		return nil, fmt.Errorf("private key is %T, want ed25519", key)
	}
	return priv, nil
}

// EncodePublicKey renders a public key as standard padded base64.
func EncodePublicKey(pub ed25519.PublicKey) string {
	return base64.StdEncoding.EncodeToString(pub)
}

// DecodePublicKey parses a standard base64 Ed25519 public key.
func DecodePublicKey(s string) (ed25519.PublicKey, error) {
	raw, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return nil, fmt.Errorf("public key is not valid base64: %w", err)
	}
	if len(raw) != ed25519.PublicKeySize {
		return nil, fmt.Errorf("public key must be %d bytes, got %d", ed25519.PublicKeySize, len(raw))
	}
	return ed25519.PublicKey(raw), nil
}

// MarshalPublicLine renders the single-line public key file:
//
//	ed25519 <base64 key> <comment>
func MarshalPublicLine(pub ed25519.PublicKey, comment string) []byte {
	line := KeyType + " " + EncodePublicKey(pub)
	if c := sanitizeComment(comment); c != "" {
		line += " " + c
	}
	return []byte(line + "\n")
}

// UnmarshalPublicLine parses the single-line public key file.
func UnmarshalPublicLine(data []byte) (ed25519.PublicKey, string, error) {
	fields := strings.Fields(string(data))
	if len(fields) < 2 {
		return nil, "", errors.New("public key file must contain `ed25519 <base64 key>`")
	}
	if fields[0] != KeyType {
		return nil, "", fmt.Errorf("unsupported key type %q, want %q", fields[0], KeyType)
	}
	pub, err := DecodePublicKey(fields[1])
	if err != nil {
		return nil, "", err
	}
	return pub, strings.Join(fields[2:], " "), nil
}

// sanitizeComment keeps the comment on one line; it is cosmetic metadata only.
func sanitizeComment(c string) string {
	c = strings.TrimSpace(c)
	c = strings.Map(func(r rune) rune {
		if r == '\n' || r == '\r' || r == '\t' {
			return ' '
		}
		return r
	}, c)
	return strings.TrimSpace(c)
}
