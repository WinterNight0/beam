package identity

import (
	"errors"
	"strings"
	"testing"
	"time"
)

func mustParse(t *testing.T, s string) *KnownPeers {
	t.Helper()
	kp, err := ParseKnownPeers([]byte(s))
	if err != nil {
		t.Fatalf("ParseKnownPeers: %v", err)
	}
	return kp
}

func TestParseKnownPeers(t *testing.T) {
	in := "# beam known_peers v1\n" +
		"\n" +
		"alice  " + KeyType + " " + vectors[0].pubB64 + "  added=2026-01-02T03:04:05Z\n" +
		"bob\t" + KeyType + "\t" + vectors[1].pubB64 + "\tadded=2026-02-03T04:05:06Z note=work-laptop\n"

	kp := mustParse(t, in)
	peers := kp.Peers()
	if len(peers) != 2 {
		t.Fatalf("got %d peers, want 2", len(peers))
	}

	alice := peers[0]
	if alice.Name != "alice" {
		t.Errorf("name = %q", alice.Name)
	}
	if alice.Fingerprint().Hex() != vectors[0].fpHex {
		t.Errorf("alice fingerprint = %s", alice.Fingerprint().Hex())
	}
	if want := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC); !alice.Added.Equal(want) {
		t.Errorf("added = %v, want %v", alice.Added, want)
	}

	bob := peers[1]
	if len(bob.Extra) != 1 || bob.Extra[0].Key != "note" || bob.Extra[0].Value != "work-laptop" {
		t.Errorf("unknown attribute was not preserved: %+v", bob.Extra)
	}
}

func TestParseKnownPeersEmpty(t *testing.T) {
	kp := mustParse(t, "")
	if kp.Len() != 0 {
		t.Fatalf("empty file yielded %d peers", kp.Len())
	}
}

func TestParseKnownPeersRejectsBadLines(t *testing.T) {
	good := "alice " + KeyType + " " + vectors[0].pubB64 + "\n"
	cases := map[string]string{
		"too few fields":    "alice " + KeyType + "\n",
		"wrong key type":    "alice ssh-rsa " + vectors[0].pubB64 + "\n",
		"bad base64":        "alice " + KeyType + " not!base64\n",
		"short key":         "alice " + KeyType + " AAAA\n",
		"bad name":          "al ice! " + KeyType + " " + vectors[0].pubB64 + "\n",
		"bare token":        good[:len(good)-1] + " trailing\n",
		"bad attr key":      good[:len(good)-1] + " bad+key=1\n",
		"bad timestamp":     good[:len(good)-1] + " added=yesterday\n",
		"duplicate name":    good + "alice " + KeyType + " " + vectors[1].pubB64 + "\n",
		"duplicate key":     good + "carol " + KeyType + " " + vectors[0].pubB64 + "\n",
		"name differs case": good + "ALICE " + KeyType + " " + vectors[1].pubB64 + "\n",
	}
	for name, in := range cases {
		_, err := ParseKnownPeers([]byte(in))
		if err == nil {
			t.Errorf("%s: parsed without error", name)
			continue
		}
		var pe *ParseError
		if !errors.As(err, &pe) {
			t.Errorf("%s: error %v is not a *ParseError", name, err)
			continue
		}
		if pe.Line < 1 {
			t.Errorf("%s: ParseError has no line number", name)
		}
	}
}

func TestRenderPreservesCommentsAndAttributes(t *testing.T) {
	in := "# beam known_peers v1\n" +
		"# hand-written note\n" +
		"\n" +
		"alice " + KeyType + " " + vectors[0].pubB64 + " added=2026-01-02T03:04:05Z note=home\n"

	out := string(mustParse(t, in).Bytes())
	for _, want := range []string{"# beam known_peers v1", "# hand-written note", "note=home", "added=2026-01-02T03:04:05Z"} {
		if !strings.Contains(out, want) {
			t.Errorf("rendered file lost %q:\n%s", want, out)
		}
	}
	// Rendering must be stable: parse(render(x)) == render(x).
	again := string(mustParse(t, out).Bytes())
	if again != out {
		t.Errorf("render is not idempotent:\n--- first ---\n%s\n--- second ---\n%s", out, again)
	}
}

func TestAdd(t *testing.T) {
	kp := mustParse(t, Header)
	alicePub, _ := testKey(t, "alpha")
	bobPub, _ := testKey(t, "bravo")

	if err := kp.Add(Peer{Name: "alice", PublicKey: alicePub}); err != nil {
		t.Fatalf("Add: %v", err)
	}
	peer, ok := kp.Lookup("alice")
	if !ok {
		t.Fatal("added peer is not found")
	}
	if peer.Added.IsZero() {
		t.Error("Add did not stamp added=")
	}

	if err := kp.Add(Peer{Name: "ALICE", PublicKey: bobPub}); !errors.Is(err, ErrPeerExists) {
		t.Errorf("adding a name that differs only in case returned %v, want ErrPeerExists", err)
	}
	if err := kp.Add(Peer{Name: "alice2", PublicKey: alicePub}); !errors.Is(err, ErrKeyInUse) {
		t.Errorf("adding a duplicate key returned %v, want ErrKeyInUse", err)
	}
	if err := kp.Add(Peer{Name: "bad name", PublicKey: bobPub}); !errors.Is(err, ErrInvalidName) {
		t.Errorf("adding an invalid name returned %v, want ErrInvalidName", err)
	}
	if err := kp.Add(Peer{Name: "short", PublicKey: []byte("too short")}); err == nil {
		t.Error("accepted a public key of the wrong length")
	}
}

func TestLookupAndLookupKey(t *testing.T) {
	kp := mustParse(t, "alice "+KeyType+" "+vectors[0].pubB64+"\n")
	if _, ok := kp.Lookup("ALICE"); !ok {
		t.Error("Lookup is not case-insensitive")
	}
	if _, ok := kp.Lookup("nobody"); ok {
		t.Error("Lookup found a peer that does not exist")
	}
	alicePub, _ := testKey(t, "alpha")
	bobPub, _ := testKey(t, "bravo")
	if p, ok := kp.LookupKey(alicePub); !ok || p.Name != "alice" {
		t.Errorf("LookupKey = %+v, %v", p, ok)
	}
	if _, ok := kp.LookupKey(bobPub); ok {
		t.Error("LookupKey matched an unknown key")
	}
}

func TestRename(t *testing.T) {
	in := "# note\nalice " + KeyType + " " + vectors[0].pubB64 + " note=home\n" +
		"bob " + KeyType + " " + vectors[1].pubB64 + "\n"
	kp := mustParse(t, in)

	if err := kp.Rename("alice", "ali"); err != nil {
		t.Fatalf("Rename: %v", err)
	}
	if _, ok := kp.Lookup("ali"); !ok {
		t.Fatal("renamed peer not found under the new name")
	}
	if _, ok := kp.Lookup("alice"); ok {
		t.Fatal("old name still resolves")
	}
	out := string(kp.Bytes())
	if !strings.Contains(out, "note=home") || !strings.Contains(out, "# note") {
		t.Errorf("rename dropped file content:\n%s", out)
	}
	if peers := kp.Peers(); peers[0].Name != "ali" {
		t.Errorf("rename changed peer order: %+v", peers)
	}

	if err := kp.Rename("ali", "bob"); !errors.Is(err, ErrPeerExists) {
		t.Errorf("renaming onto an existing name returned %v, want ErrPeerExists", err)
	}
	if err := kp.Rename("nobody", "x"); !errors.Is(err, ErrPeerNotFound) {
		t.Errorf("renaming an unknown peer returned %v, want ErrPeerNotFound", err)
	}
	if err := kp.Rename("ali", "not a name"); !errors.Is(err, ErrInvalidName) {
		t.Errorf("renaming to an invalid name returned %v, want ErrInvalidName", err)
	}
	// Changing only the capitalisation of a peer's own name is allowed.
	if err := kp.Rename("ali", "Ali"); err != nil {
		t.Errorf("recapitalising a name returned %v", err)
	}
}

func TestRemove(t *testing.T) {
	in := "# note\nalice " + KeyType + " " + vectors[0].pubB64 + "\n" +
		"bob " + KeyType + " " + vectors[1].pubB64 + "\n"
	kp := mustParse(t, in)

	if err := kp.Remove("ALICE"); err != nil {
		t.Fatalf("Remove: %v", err)
	}
	if kp.Len() != 1 {
		t.Fatalf("got %d peers after remove, want 1", kp.Len())
	}
	if _, ok := kp.Lookup("alice"); ok {
		t.Error("removed peer still resolves")
	}
	if !strings.Contains(string(kp.Bytes()), "# note") {
		t.Error("remove dropped a comment line")
	}
	if err := kp.Remove("alice"); !errors.Is(err, ErrPeerNotFound) {
		t.Errorf("removing an unknown peer returned %v, want ErrPeerNotFound", err)
	}
}

func TestValidateName(t *testing.T) {
	valid := []string{"a", "alice", "Alice-2", "my.laptop", "under_score", strings.Repeat("x", 32)}
	for _, n := range valid {
		if err := ValidateName(n); err != nil {
			t.Errorf("ValidateName(%q) = %v, want nil", n, err)
		}
	}
	invalid := []string{"", " ", "with space", "emoji\u2728", "slash/name", "hash#name", strings.Repeat("x", 33)}
	for _, n := range invalid {
		if err := ValidateName(n); err == nil {
			t.Errorf("ValidateName(%q) accepted an invalid name", n)
		}
	}
}
