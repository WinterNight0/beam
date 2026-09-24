package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"beam/internal/identity"
)

type result struct {
	code   int
	stdout string
	stderr string
}

// run executes the command tree against an isolated beam home directory.
func run(t *testing.T, dir, stdin string, args ...string) result {
	t.Helper()
	var out, errOut bytes.Buffer
	code := Execute(append([]string{"--beam-dir", dir}, args...), strings.NewReader(stdin), &out, &errOut)
	return result{code: code, stdout: out.String(), stderr: errOut.String()}
}

func beamDir(t *testing.T) string {
	t.Helper()
	return filepath.Join(t.TempDir(), ".beam")
}

// initialised returns a beam home directory that already has an identity.
func initialised(t *testing.T) string {
	t.Helper()
	dir := beamDir(t)
	if r := run(t, dir, "", "init"); r.code != ExitOK {
		t.Fatalf("init failed: code=%d stderr=%s", r.code, r.stderr)
	}
	return dir
}

func TestInitCreatesIdentity(t *testing.T) {
	dir := beamDir(t)
	r := run(t, dir, "", "init")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	for _, want := range []string{"Short ID", "Fingerprint", "SHA256:"} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("output is missing %q:\n%s", want, r.stdout)
		}
	}
	for _, name := range []string{identity.PrivateKeyName, identity.PublicKeyName, identity.KnownPeersName} {
		if _, err := os.Stat(filepath.Join(dir, name)); err != nil {
			t.Errorf("%s was not created: %v", name, err)
		}
	}
}

func TestInitRefusesToOverwrite(t *testing.T) {
	dir := initialised(t)
	before := run(t, dir, "", "whoami", "--json").stdout

	r := run(t, dir, "", "init")
	if r.code != ExitError {
		t.Fatalf("second init: code = %d, want %d", r.code, ExitError)
	}
	if !strings.Contains(r.stderr, "already exists") {
		t.Errorf("unhelpful error: %s", r.stderr)
	}
	if after := run(t, dir, "", "whoami", "--json").stdout; after != before {
		t.Error("the identity changed even though init failed")
	}

	if r := run(t, dir, "", "init", "--force"); r.code != ExitOK {
		t.Fatalf("init --force: code = %d, stderr = %s", r.code, r.stderr)
	}
	if after := run(t, dir, "", "whoami", "--json").stdout; after == before {
		t.Error("init --force did not generate a new key")
	}
}

func TestWhoami(t *testing.T) {
	dir := initialised(t)
	r := run(t, dir, "", "whoami", "--json")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	var self struct {
		ShortID     string `json:"short_id"`
		Fingerprint string `json:"fingerprint"`
		KeyType     string `json:"key_type"`
		PublicKey   string `json:"public_key"`
		Dir         string `json:"dir"`
	}
	if err := json.Unmarshal([]byte(r.stdout), &self); err != nil {
		t.Fatalf("output is not JSON: %v\n%s", err, r.stdout)
	}
	if len(self.ShortID) != 9 {
		t.Errorf("short_id = %q", self.ShortID)
	}
	if self.KeyType != identity.KeyType {
		t.Errorf("key_type = %q", self.KeyType)
	}
	if self.Dir != dir {
		t.Errorf("dir = %q, want %q", self.Dir, dir)
	}

	// The JSON must describe the key actually stored on disk.
	pub, err := identity.DecodePublicKey(self.PublicKey)
	if err != nil {
		t.Fatalf("public_key is not a valid key: %v", err)
	}
	if got := identity.FingerprintOf(pub).String(); got != self.Fingerprint {
		t.Errorf("fingerprint = %s, want %s", self.Fingerprint, got)
	}
}

func TestWhoamiWithoutIdentity(t *testing.T) {
	r := run(t, beamDir(t), "", "whoami")
	if r.code != ExitError {
		t.Fatalf("code = %d, want %d", r.code, ExitError)
	}
	if !strings.Contains(r.stderr, "beam init") {
		t.Errorf("error does not point at `beam init`: %s", r.stderr)
	}
}

// seedPeers writes a known_peers file with two entries.
func seedPeers(t *testing.T, dir string) {
	t.Helper()
	store := identity.NewStore(dir)
	kp, err := store.LoadKnownPeers()
	if err != nil {
		t.Fatalf("LoadKnownPeers: %v", err)
	}
	for _, name := range []string{"alice", "bob"} {
		id, err := identity.Generate(name)
		if err != nil {
			t.Fatalf("Generate: %v", err)
		}
		if err := kp.Add(identity.Peer{Name: name, PublicKey: id.Public}); err != nil {
			t.Fatalf("Add %s: %v", name, err)
		}
	}
	if err := store.SaveKnownPeers(kp); err != nil {
		t.Fatalf("SaveKnownPeers: %v", err)
	}
}

func TestPeersEmpty(t *testing.T) {
	r := run(t, initialised(t), "", "peers")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	if !strings.Contains(r.stdout, "No paired peers") {
		t.Errorf("unexpected output:\n%s", r.stdout)
	}
}

func TestPeersList(t *testing.T) {
	dir := initialised(t)
	seedPeers(t, dir)

	r := run(t, dir, "", "peers")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	for _, want := range []string{"NAME", "FINGERPRINT", "alice", "bob", "SHA256:"} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("table is missing %q:\n%s", want, r.stdout)
		}
	}

	r = run(t, dir, "", "peers", "--json")
	var list []struct {
		Name        string `json:"name"`
		ShortID     string `json:"short_id"`
		Fingerprint string `json:"fingerprint"`
		Added       string `json:"added"`
	}
	if err := json.Unmarshal([]byte(r.stdout), &list); err != nil {
		t.Fatalf("output is not JSON: %v\n%s", err, r.stdout)
	}
	if len(list) != 2 || list[0].Name != "alice" || list[1].Name != "bob" {
		t.Fatalf("unexpected peer list: %+v", list)
	}
	if list[0].Added == "" {
		t.Error("added timestamp is missing")
	}
}

func TestPeersReportsCorruptFile(t *testing.T) {
	dir := initialised(t)
	bad := identity.Header + "alice ed25519 not-base64!\n"
	if err := os.WriteFile(filepath.Join(dir, identity.KnownPeersName), []byte(bad), 0o600); err != nil {
		t.Fatalf("write known_peers: %v", err)
	}
	r := run(t, dir, "", "peers")
	if r.code != ExitError {
		t.Fatalf("code = %d, want %d", r.code, ExitError)
	}
	if !strings.Contains(r.stderr, "line 3") {
		t.Errorf("error does not name the bad line: %s", r.stderr)
	}
}

func TestRenameCommand(t *testing.T) {
	dir := initialised(t)
	seedPeers(t, dir)

	if r := run(t, dir, "", "rename", "alice", "ali"); r.code != ExitOK {
		t.Fatalf("rename: code = %d, stderr = %s", r.code, r.stderr)
	}
	peers := run(t, dir, "", "peers").stdout
	if !strings.Contains(peers, "ali") || strings.Contains(peers, "alice") {
		t.Errorf("rename did not take effect:\n%s", peers)
	}

	if r := run(t, dir, "", "rename", "nobody", "x"); r.code != ExitError {
		t.Errorf("renaming an unknown peer: code = %d", r.code)
	}
	if r := run(t, dir, "", "rename", "ali", "bob"); r.code != ExitError {
		t.Errorf("renaming onto an existing name: code = %d", r.code)
	}
	if r := run(t, dir, "", "rename", "ali"); r.code != ExitError {
		t.Errorf("rename with one argument: code = %d", r.code)
	}
}

func TestRemoveCommand(t *testing.T) {
	dir := initialised(t)
	seedPeers(t, dir)

	// Declining the prompt keeps the peer.
	r := run(t, dir, "n\n", "remove", "alice")
	if r.code != ExitOK {
		t.Fatalf("remove (declined): code = %d, stderr = %s", r.code, r.stderr)
	}
	if !strings.Contains(r.stdout, "Cancelled") {
		t.Errorf("unexpected output:\n%s", r.stdout)
	}
	if !strings.Contains(run(t, dir, "", "peers").stdout, "alice") {
		t.Fatal("peer was removed after the prompt was declined")
	}

	// End of input is also a no.
	if r := run(t, dir, "", "remove", "alice"); r.code != ExitOK {
		t.Fatalf("remove (EOF): code = %d", r.code)
	}
	if !strings.Contains(run(t, dir, "", "peers").stdout, "alice") {
		t.Fatal("peer was removed when the prompt got no answer")
	}

	// Confirming removes it.
	if r := run(t, dir, "y\n", "remove", "alice"); r.code != ExitOK {
		t.Fatalf("remove (confirmed): code = %d, stderr = %s", r.code, r.stderr)
	}
	peers := run(t, dir, "", "peers").stdout
	if strings.Contains(peers, "alice") {
		t.Errorf("peer was not removed:\n%s", peers)
	}
	if !strings.Contains(peers, "bob") {
		t.Errorf("remove deleted the wrong peer:\n%s", peers)
	}

	if r := run(t, dir, "", "remove", "bob", "--yes"); r.code != ExitOK {
		t.Fatalf("remove --yes: code = %d, stderr = %s", r.code, r.stderr)
	}
	if r := run(t, dir, "", "remove", "nobody", "--yes"); r.code != ExitError {
		t.Errorf("removing an unknown peer: code = %d", r.code)
	}
}

func TestRemovePromptShowsFingerprint(t *testing.T) {
	dir := initialised(t)
	seedPeers(t, dir)
	r := run(t, dir, "n\n", "remove", "alice")
	if !strings.Contains(r.stdout, "SHA256:") {
		t.Errorf("the confirmation prompt does not show the fingerprint:\n%s", r.stdout)
	}
}

func TestStubbedCommandsExitTwo(t *testing.T) {
	dir := initialised(t)
	cases := [][]string{
		{"listen"},
		{"send", "alice", "file.txt"},
		{"pair", "123456789", "--name", "alice"},
		{"newcode"},
	}
	for _, args := range cases {
		r := run(t, dir, "", args...)
		if r.code != ExitNotImplemented {
			t.Errorf("%v: code = %d, want %d (stderr: %s)", args, r.code, ExitNotImplemented, r.stderr)
		}
		if !strings.Contains(r.stderr, "not implemented yet") {
			t.Errorf("%v: unhelpful error: %s", args, r.stderr)
		}
	}
}

func TestUnknownCommand(t *testing.T) {
	r := run(t, beamDir(t), "", "definitely-not-a-command")
	if r.code != ExitError {
		t.Fatalf("code = %d, want %d", r.code, ExitError)
	}
}

func TestVersion(t *testing.T) {
	r := run(t, beamDir(t), "", "version")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	if !strings.HasPrefix(r.stdout, "beam ") {
		t.Errorf("unexpected output: %q", r.stdout)
	}
}

func TestHelpListsEveryCommand(t *testing.T) {
	r := run(t, beamDir(t), "", "--help")
	if r.code != ExitOK {
		t.Fatalf("code = %d, stderr = %s", r.code, r.stderr)
	}
	for _, name := range []string{"init", "whoami", "peers", "pair", "rename", "remove", "listen", "send", "newcode"} {
		if !strings.Contains(r.stdout, name) {
			t.Errorf("help does not mention %q:\n%s", name, r.stdout)
		}
	}
}
