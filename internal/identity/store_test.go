package identity

import (
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func newTestStore(t *testing.T) *Store {
	t.Helper()
	return NewStore(filepath.Join(t.TempDir(), ".beam"))
}

func TestDefaultDirHonoursEnv(t *testing.T) {
	t.Setenv(DirEnv, filepath.Join("some", "where"))
	dir, err := DefaultDir()
	if err != nil {
		t.Fatalf("DefaultDir: %v", err)
	}
	if dir != filepath.Join("some", "where") {
		t.Fatalf("DefaultDir = %q", dir)
	}

	t.Setenv(DirEnv, "")
	dir, err = DefaultDir()
	if err != nil {
		t.Fatalf("DefaultDir: %v", err)
	}
	if !strings.HasSuffix(dir, dirName) {
		t.Fatalf("DefaultDir = %q, want a path ending in %q", dir, dirName)
	}
}

func TestSaveAndLoadIdentity(t *testing.T) {
	s := newTestStore(t)
	if s.HasIdentity() {
		t.Fatal("a fresh store reports an existing identity")
	}

	id, err := Generate("laptop")
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if err := s.SaveIdentity(id, false); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}
	if !s.HasIdentity() {
		t.Fatal("identity not found after save")
	}

	loaded, err := s.LoadIdentity()
	if err != nil {
		t.Fatalf("LoadIdentity: %v", err)
	}
	if !loaded.Private.Equal(id.Private) {
		t.Error("loaded private key differs")
	}
	if loaded.Fingerprint() != id.Fingerprint() {
		t.Error("loaded fingerprint differs")
	}
	if loaded.Comment != "laptop" {
		t.Errorf("comment = %q, want %q", loaded.Comment, "laptop")
	}

	// init also seeds an empty known_peers and the tmp directory.
	if _, err := os.Stat(s.KnownPeersPath()); err != nil {
		t.Errorf("known_peers was not created: %v", err)
	}
	if info, err := os.Stat(s.TmpPath()); err != nil || !info.IsDir() {
		t.Errorf("tmp directory was not created: %v", err)
	}
}

func TestSaveIdentityRefusesOverwrite(t *testing.T) {
	s := newTestStore(t)
	first, _ := Generate("one")
	if err := s.SaveIdentity(first, false); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}

	second, _ := Generate("two")
	err := s.SaveIdentity(second, false)
	if !errors.Is(err, ErrIdentityExists) {
		t.Fatalf("SaveIdentity over an existing key returned %v, want ErrIdentityExists", err)
	}
	loaded, err := s.LoadIdentity()
	if err != nil {
		t.Fatalf("LoadIdentity: %v", err)
	}
	if loaded.Fingerprint() != first.Fingerprint() {
		t.Fatal("the original key was replaced despite the error")
	}

	if err := s.SaveIdentity(second, true); err != nil {
		t.Fatalf("SaveIdentity(force): %v", err)
	}
	loaded, err = s.LoadIdentity()
	if err != nil {
		t.Fatalf("LoadIdentity: %v", err)
	}
	if loaded.Fingerprint() != second.Fingerprint() {
		t.Fatal("--force did not replace the key")
	}
}

func TestSaveIdentityKeepsExistingKnownPeers(t *testing.T) {
	s := newTestStore(t)
	if err := s.EnsureDirs(); err != nil {
		t.Fatalf("EnsureDirs: %v", err)
	}
	existing := Header + "alice " + KeyType + " " + vectors[0].pubB64 + "\n"
	if err := os.WriteFile(s.KnownPeersPath(), []byte(existing), 0o600); err != nil {
		t.Fatalf("seed known_peers: %v", err)
	}

	id, _ := Generate("laptop")
	if err := s.SaveIdentity(id, true); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}
	kp, err := s.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	if _, ok := kp.Lookup("alice"); !ok {
		t.Fatal("re-running init destroyed known_peers")
	}
}

func TestLoadIdentityMissing(t *testing.T) {
	s := newTestStore(t)
	if _, err := s.LoadIdentity(); !errors.Is(err, ErrNoIdentity) {
		t.Fatalf("LoadIdentity on an empty store returned %v, want ErrNoIdentity", err)
	}
}

func TestLoadIdentityDetectsMismatchedPublicKey(t *testing.T) {
	s := newTestStore(t)
	id, _ := Generate("laptop")
	if err := s.SaveIdentity(id, false); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}
	other, _ := Generate("someone else")
	if err := os.WriteFile(s.PublicKeyPath(), MarshalPublicLine(other.Public, "swapped"), 0o644); err != nil {
		t.Fatalf("overwrite public key: %v", err)
	}
	if _, err := s.LoadIdentity(); err == nil {
		t.Fatal("a public key that does not match the private key was accepted")
	}
}

func TestLoadIdentityToleratesMissingPublicKeyFile(t *testing.T) {
	s := newTestStore(t)
	id, _ := Generate("laptop")
	if err := s.SaveIdentity(id, false); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}
	if err := os.Remove(s.PublicKeyPath()); err != nil {
		t.Fatalf("remove public key: %v", err)
	}
	loaded, err := s.LoadIdentity()
	if err != nil {
		t.Fatalf("LoadIdentity: %v", err)
	}
	if loaded.Fingerprint() != id.Fingerprint() {
		t.Fatal("public key was not re-derived from the private key")
	}
}

func TestPrivateKeyPermissions(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("Unix permission bits are not meaningful on Windows; see ADR-0004")
	}
	s := newTestStore(t)
	id, _ := Generate("laptop")
	if err := s.SaveIdentity(id, false); err != nil {
		t.Fatalf("SaveIdentity: %v", err)
	}
	for _, p := range []string{s.PrivateKeyPath(), s.KnownPeersPath()} {
		info, err := os.Stat(p)
		if err != nil {
			t.Fatalf("stat %s: %v", p, err)
		}
		if mode := info.Mode().Perm(); mode&0o077 != 0 {
			t.Errorf("%s has mode %04o, want no group/other access", p, mode)
		}
	}
	if w := s.PermissionWarnings(); len(w) != 0 {
		t.Errorf("unexpected permission warnings: %v", w)
	}
}

func TestLoadKnownPeersMissingFile(t *testing.T) {
	s := newTestStore(t)
	kp, err := s.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	if kp.Len() != 0 {
		t.Fatalf("got %d peers from a missing file", kp.Len())
	}
}

func TestLoadKnownPeersReportsCorruption(t *testing.T) {
	s := newTestStore(t)
	if err := s.EnsureDirs(); err != nil {
		t.Fatalf("EnsureDirs: %v", err)
	}
	bad := Header + "alice " + KeyType + " not-base64!\n"
	if err := os.WriteFile(s.KnownPeersPath(), []byte(bad), 0o600); err != nil {
		t.Fatalf("write known_peers: %v", err)
	}
	_, err := s.LoadKnownPeers()
	if err == nil {
		t.Fatal("a corrupt known_peers file was accepted")
	}
	var pe *ParseError
	if !errors.As(err, &pe) {
		t.Fatalf("error %v does not carry a line number", err)
	}
	if !strings.Contains(err.Error(), s.KnownPeersPath()) {
		t.Errorf("error does not name the file: %v", err)
	}
}

func TestSaveKnownPeersRoundTrip(t *testing.T) {
	s := newTestStore(t)
	kp, err := s.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	pub, _ := testKey(t, "alpha")
	if err := kp.Add(Peer{Name: "alice", PublicKey: pub}); err != nil {
		t.Fatalf("Add: %v", err)
	}
	if err := s.SaveKnownPeers(kp); err != nil {
		t.Fatalf("SaveKnownPeers: %v", err)
	}

	data, err := os.ReadFile(s.KnownPeersPath())
	if err != nil {
		t.Fatalf("read known_peers: %v", err)
	}
	if strings.Contains(string(data), "\r\n") {
		t.Error("known_peers was written with CRLF line endings")
	}
	if !strings.HasPrefix(string(data), "# beam known_peers v1") {
		t.Errorf("known_peers is missing its header:\n%s", data)
	}

	reloaded, err := s.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	if _, ok := reloaded.Lookup("alice"); !ok {
		t.Fatal("peer did not survive the round trip")
	}
}

func TestSaveKnownPeersLeavesNoTempFiles(t *testing.T) {
	s := newTestStore(t)
	kp, err := s.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	if err := s.SaveKnownPeers(kp); err != nil {
		t.Fatalf("SaveKnownPeers: %v", err)
	}
	entries, err := os.ReadDir(s.Dir())
	if err != nil {
		t.Fatalf("read dir: %v", err)
	}
	for _, e := range entries {
		if strings.Contains(e.Name(), ".tmp") {
			t.Errorf("left a temporary file behind: %s", e.Name())
		}
	}
}
