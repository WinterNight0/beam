package identity

import (
	"crypto/ed25519"
	"crypto/sha256"
	"testing"
)

// Fixed vectors, derived from deterministic seeds so that any change to the
// fingerprint or short-ID derivation breaks the build rather than silently
// changing every user's identifier.
var vectors = []struct {
	name    string
	pubB64  string
	fpHex   string
	shortID string
}{
	{
		name:    "alpha",
		pubB64:  "4V1sbBWRwKcMoCdgmMZSy3enESln8Qgij/DzRjafNjs=",
		fpHex:   "14e041bc27b36219678cbb5d9c40c38a41b634bd03c25e1c514d1d95ab8536de",
		shortID: "917470233",
	},
	{
		name:    "bravo",
		pubB64:  "K0yzICzxhJcR7D2z5/JxBpS2pmFS7rGHZ6/DfXJkpD0=",
		fpHex:   "2cfc94c48f5c67e1b942bddc3127eed3fd109289e17e65df0676299c656005e7",
		shortID: "739613153",
	},
}

// testKey returns the deterministic keypair used by the vectors above.
func testKey(t *testing.T, name string) (ed25519.PublicKey, ed25519.PrivateKey) {
	t.Helper()
	seed := sha256.Sum256([]byte("beam-test-vector-" + name))
	priv := ed25519.NewKeyFromSeed(seed[:])
	return priv.Public().(ed25519.PublicKey), priv
}

func TestFingerprintAndShortIDVectors(t *testing.T) {
	for _, v := range vectors {
		t.Run(v.name, func(t *testing.T) {
			pub, _ := testKey(t, v.name)
			if got := EncodePublicKey(pub); got != v.pubB64 {
				t.Fatalf("public key = %s, want %s", got, v.pubB64)
			}
			fp := FingerprintOf(pub)
			if got := fp.Hex(); got != v.fpHex {
				t.Errorf("fingerprint = %s, want %s", got, v.fpHex)
			}
			if got := fp.String(); got != "SHA256:"+v.fpHex {
				t.Errorf("String() = %s, want SHA256:%s", got, v.fpHex)
			}
			if got := fp.Short(); got != "SHA256:"+v.fpHex[:16]+"..." {
				t.Errorf("Short() = %s", got)
			}
			if got := fp.ShortID().String(); got != v.shortID {
				t.Errorf("short ID = %s, want %s", got, v.shortID)
			}
		})
	}
}

func TestShortIDIsNineDigits(t *testing.T) {
	// Any fingerprint must map into the 9-digit space, including the extremes.
	for _, fp := range []Fingerprint{{}, {0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff}} {
		id := fp.ShortID()
		if uint32(id) >= shortIDModulus {
			t.Fatalf("short ID %d out of range", id)
		}
		if got := id.String(); len(got) != 9 {
			t.Fatalf("short ID %q has %d digits, want 9", got, len(got))
		}
	}
}

func TestShortIDDisplay(t *testing.T) {
	if got := ShortID(4815162).Display(); got != "004 815 162" {
		t.Errorf("Display() = %q, want %q", got, "004 815 162")
	}
}

func TestParseShortID(t *testing.T) {
	valid := map[string]ShortID{
		"004815162":   4815162,
		"004 815 162": 4815162,
		"004-815-162": 4815162,
		"917470233":   917470233,
	}
	for in, want := range valid {
		got, err := ParseShortID(in)
		if err != nil {
			t.Errorf("ParseShortID(%q) returned error: %v", in, err)
			continue
		}
		if got != want {
			t.Errorf("ParseShortID(%q) = %d, want %d", in, got, want)
		}
	}
	for _, in := range []string{"", "12345678", "1234567890", "12345678a", "004.815.162"} {
		if _, err := ParseShortID(in); err == nil {
			t.Errorf("ParseShortID(%q) accepted an invalid short ID", in)
		}
	}
}

func TestParseFingerprint(t *testing.T) {
	v := vectors[0]
	forms := []string{
		v.fpHex,
		"SHA256:" + v.fpHex,
		"sha256:" + v.fpHex,
		"14:e0:41:bc:27:b3:62:19:67:8c:bb:5d:9c:40:c3:8a:41:b6:34:bd:03:c2:5e:1c:51:4d:1d:95:ab:85:36:de",
	}
	for _, form := range forms {
		fp, err := ParseFingerprint(form)
		if err != nil {
			t.Errorf("ParseFingerprint(%q) returned error: %v", form, err)
			continue
		}
		if fp.Hex() != v.fpHex {
			t.Errorf("ParseFingerprint(%q) = %s", form, fp.Hex())
		}
	}
	for _, bad := range []string{"", "abc", v.fpHex + "00", "zz" + v.fpHex[2:]} {
		if _, err := ParseFingerprint(bad); err == nil {
			t.Errorf("ParseFingerprint(%q) accepted an invalid fingerprint", bad)
		}
	}
}
