package identity

import (
	"bytes"
	"crypto/ed25519"
	"crypto/subtle"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
)

// File and directory names inside the beam home directory.
const (
	// DirEnv overrides the beam home directory; mainly used by tests.
	DirEnv = "BEAM_DIR"

	dirName        = ".beam"
	PrivateKeyName = "id_ed25519"
	PublicKeyName  = "id_ed25519.pub"
	KnownPeersName = "known_peers"
	TmpName        = "tmp"

	dirMode         = 0o700
	privateFileMode = 0o600
	publicFileMode  = 0o644
)

// ErrIdentityExists is returned when init would overwrite an existing keypair.
var ErrIdentityExists = errors.New("an identity already exists on this device")

// DefaultDir resolves the beam home directory, honouring $BEAM_DIR.
func DefaultDir() (string, error) {
	if d := os.Getenv(DirEnv); d != "" {
		return d, nil
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("locate home directory: %w", err)
	}
	return filepath.Join(home, dirName), nil
}

// Store is the on-disk identity state under the beam home directory.
type Store struct {
	dir string
}

// NewStore returns a Store rooted at dir.
func NewStore(dir string) *Store { return &Store{dir: dir} }

// Dir returns the beam home directory.
func (s *Store) Dir() string { return s.dir }

// PrivateKeyPath is the path of the device private key.
func (s *Store) PrivateKeyPath() string { return filepath.Join(s.dir, PrivateKeyName) }

// PublicKeyPath is the path of the device public key.
func (s *Store) PublicKeyPath() string { return filepath.Join(s.dir, PublicKeyName) }

// KnownPeersPath is the path of the known_peers database.
func (s *Store) KnownPeersPath() string { return filepath.Join(s.dir, KnownPeersName) }

// TmpPath is the directory for in-progress transfers (used from M2 onwards).
func (s *Store) TmpPath() string { return filepath.Join(s.dir, TmpName) }

// EnsureDirs creates the beam home directory and its tmp subdirectory.
func (s *Store) EnsureDirs() error {
	for _, d := range []string{s.dir, s.TmpPath()} {
		if err := os.MkdirAll(d, dirMode); err != nil {
			return fmt.Errorf("create %s: %w", d, err)
		}
	}
	return nil
}

// HasIdentity reports whether a private key is already present.
func (s *Store) HasIdentity() bool {
	_, err := os.Stat(s.PrivateKeyPath())
	return err == nil
}

// SaveIdentity writes the keypair. Unless force is set it refuses to overwrite
// an existing key, because replacing a key silently would invalidate every
// pairing other peers hold for this device.
func (s *Store) SaveIdentity(id *Identity, force bool) error {
	if !force && s.HasIdentity() {
		return fmt.Errorf("%w: %s", ErrIdentityExists, s.PrivateKeyPath())
	}
	if err := s.EnsureDirs(); err != nil {
		return err
	}
	privPEM, err := MarshalPrivateKey(id.Private)
	if err != nil {
		return err
	}
	if err := writeFileAtomic(s.PrivateKeyPath(), privPEM, privateFileMode); err != nil {
		return fmt.Errorf("write private key: %w", err)
	}
	pubLine := MarshalPublicLine(id.Public, id.Comment)
	if err := writeFileAtomic(s.PublicKeyPath(), pubLine, publicFileMode); err != nil {
		return fmt.Errorf("write public key: %w", err)
	}
	if _, err := os.Stat(s.KnownPeersPath()); errors.Is(err, os.ErrNotExist) {
		if err := writeFileAtomic(s.KnownPeersPath(), []byte(Header), privateFileMode); err != nil {
			return fmt.Errorf("create known_peers: %w", err)
		}
	}
	return nil
}

// LoadIdentity reads the device keypair. It verifies that the stored public
// key file matches the private key; a mismatch means the directory has been
// tampered with or corrupted and is never repaired silently.
func (s *Store) LoadIdentity() (*Identity, error) {
	data, err := os.ReadFile(s.PrivateKeyPath())
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, ErrNoIdentity
		}
		return nil, fmt.Errorf("read private key: %w", err)
	}
	priv, err := UnmarshalPrivateKey(data)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", s.PrivateKeyPath(), err)
	}
	pub, ok := priv.Public().(ed25519.PublicKey)
	if !ok {
		return nil, errors.New("private key does not yield an ed25519 public key")
	}
	id := &Identity{Private: priv, Public: pub}

	pubData, err := os.ReadFile(s.PublicKeyPath())
	switch {
	case errors.Is(err, os.ErrNotExist):
		// Tolerated: the public half is derivable from the private key.
	case err != nil:
		return nil, fmt.Errorf("read public key: %w", err)
	default:
		filePub, comment, err := UnmarshalPublicLine(pubData)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", s.PublicKeyPath(), err)
		}
		if subtle.ConstantTimeCompare(filePub, pub) != 1 {
			return nil, fmt.Errorf("%s does not match %s; refusing to guess which one is yours",
				s.PublicKeyPath(), s.PrivateKeyPath())
		}
		id.Comment = comment
	}
	return id, nil
}

// LoadKnownPeers reads the peer database. A missing file is an empty database.
func (s *Store) LoadKnownPeers() (*KnownPeers, error) {
	data, err := os.ReadFile(s.KnownPeersPath())
	if errors.Is(err, os.ErrNotExist) {
		return ParseKnownPeers([]byte(Header))
	}
	if err != nil {
		return nil, fmt.Errorf("read known_peers: %w", err)
	}
	kp, err := ParseKnownPeers(data)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", s.KnownPeersPath(), err)
	}
	return kp, nil
}

// SaveKnownPeers writes the peer database atomically.
func (s *Store) SaveKnownPeers(kp *KnownPeers) error {
	if err := s.EnsureDirs(); err != nil {
		return err
	}
	data := kp.Bytes()
	if !bytes.HasPrefix(data, []byte("#")) {
		data = append([]byte(Header), data...)
	}
	if err := writeFileAtomic(s.KnownPeersPath(), data, privateFileMode); err != nil {
		return fmt.Errorf("write known_peers: %w", err)
	}
	return nil
}

// PermissionWarnings reports private files that are readable by other users.
// On Windows the Unix mode bits are not meaningful, so the check is skipped;
// see docs/decisions.md ADR-0004.
func (s *Store) PermissionWarnings() []string {
	if runtime.GOOS == "windows" {
		return nil
	}
	var warnings []string
	for _, p := range []string{s.PrivateKeyPath(), s.KnownPeersPath()} {
		info, err := os.Stat(p)
		if err != nil {
			continue
		}
		if mode := info.Mode().Perm(); mode&0o077 != 0 {
			warnings = append(warnings, fmt.Sprintf("%s has permissions %04o; expected 0600", p, mode))
		}
	}
	return warnings
}

// writeFileAtomic writes data to a temporary file in the same directory and
// renames it into place, so a crash mid-write cannot leave a truncated file.
func writeFileAtomic(path string, data []byte, perm os.FileMode) error {
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, filepath.Base(path)+".tmp*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)

	if err := tmp.Chmod(perm); err != nil && runtime.GOOS != "windows" {
		tmp.Close()
		return err
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmpName, path)
}
