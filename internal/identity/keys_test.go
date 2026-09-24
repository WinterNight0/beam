package identity

import (
	"bytes"
	"crypto/ed25519"
	"strings"
	"testing"
)

func TestGenerateProducesUsableKeypair(t *testing.T) {
	id, err := Generate("laptop")
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if len(id.Private) != ed25519.PrivateKeySize || len(id.Public) != ed25519.PublicKeySize {
		t.Fatalf("unexpected key sizes: priv=%d pub=%d", len(id.Private), len(id.Public))
	}
	msg := []byte("beam")
	if !ed25519.Verify(id.Public, msg, ed25519.Sign(id.Private, msg)) {
		t.Fatal("public key does not verify a signature made with the private key")
	}
	if id.Comment != "laptop" {
		t.Errorf("comment = %q, want %q", id.Comment, "laptop")
	}
}

func TestGenerateIsRandom(t *testing.T) {
	a, err := Generate("")
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	b, err := Generate("")
	if err != nil {
		t.Fatalf("Generate: %v", err)
	}
	if a.Fingerprint() == b.Fingerprint() {
		t.Fatal("two generated identities share a fingerprint")
	}
}

func TestPrivateKeyRoundTrip(t *testing.T) {
	_, priv := testKey(t, "alpha")
	pem, err := MarshalPrivateKey(priv)
	if err != nil {
		t.Fatalf("MarshalPrivateKey: %v", err)
	}
	if !bytes.HasPrefix(pem, []byte("-----BEGIN BEAM PRIVATE KEY-----")) {
		t.Fatalf("unexpected PEM header: %q", string(pem[:40]))
	}
	got, err := UnmarshalPrivateKey(pem)
	if err != nil {
		t.Fatalf("UnmarshalPrivateKey: %v", err)
	}
	if !got.Equal(priv) {
		t.Fatal("round-tripped private key differs from the original")
	}
}

func TestUnmarshalPrivateKeyRejectsGarbage(t *testing.T) {
	cases := map[string][]byte{
		"empty":      nil,
		"not pem":    []byte("just some text"),
		"wrong type": []byte("-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n"),
		"bad der":    []byte("-----BEGIN BEAM PRIVATE KEY-----\nAAAA\n-----END BEAM PRIVATE KEY-----\n"),
	}
	for name, data := range cases {
		if _, err := UnmarshalPrivateKey(data); err == nil {
			t.Errorf("%s: accepted an invalid private key", name)
		}
	}
}

func TestPublicLineRoundTrip(t *testing.T) {
	pub, _ := testKey(t, "bravo")
	line := MarshalPublicLine(pub, "desktop")
	want := KeyType + " " + vectors[1].pubB64 + " desktop\n"
	if string(line) != want {
		t.Fatalf("public line = %q, want %q", line, want)
	}
	got, comment, err := UnmarshalPublicLine(line)
	if err != nil {
		t.Fatalf("UnmarshalPublicLine: %v", err)
	}
	if !got.Equal(pub) {
		t.Error("round-tripped public key differs from the original")
	}
	if comment != "desktop" {
		t.Errorf("comment = %q, want %q", comment, "desktop")
	}
}

func TestMarshalPublicLineKeepsOneLine(t *testing.T) {
	pub, _ := testKey(t, "alpha")
	line := string(MarshalPublicLine(pub, "my\nhost\tname "))
	if strings.Count(line, "\n") != 1 || !strings.HasSuffix(line, "\n") {
		t.Fatalf("comment leaked a newline into the public key file: %q", line)
	}
}

func TestUnmarshalPublicLineRejectsGarbage(t *testing.T) {
	cases := map[string]string{
		"empty":        "",
		"missing key":  "ed25519\n",
		"wrong type":   "ssh-rsa " + vectors[0].pubB64 + "\n",
		"bad base64":   "ed25519 not-base64!!\n",
		"short key":    "ed25519 AAAA\n",
		"only comment": "comment only\n",
	}
	for name, data := range cases {
		if _, _, err := UnmarshalPublicLine([]byte(data)); err == nil {
			t.Errorf("%s: accepted an invalid public key line", name)
		}
	}
}

func TestDecodePublicKeyRejectsWrongLength(t *testing.T) {
	if _, err := DecodePublicKey("AAAAAAAA"); err == nil {
		t.Fatal("accepted a public key of the wrong length")
	}
}
