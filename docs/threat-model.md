# Threat model

What beam protects, from whom, how, and what it deliberately does not. Every
mitigation below names the decision behind it (ADR, in `decisions.md`) and the
test that demonstrates it; a claim without a test is marked as such.

Written for M6, against the code at that milestone: iroh `=1.2.0` as the
transport, `beam-server` as the rendezvous, n0's relay as the default relay.

## 1. System in one paragraph

Two devices each hold an Ed25519 key (`~/.beam/id_ed25519`). They pair once:
one runs `beam listen`, the other `beam pair <Short ID>`; SPAKE2 over an iroh
connection proves both know a six-digit code, and each person confirms the
other's fingerprint with `yes`. Afterwards each device keeps the other's public
key in `known_peers`. `beam send` finds the peer by that key through the
rendezvous server and connects over iroh (QUIC/TLS, keyed by the same Ed25519
identities — so the connection itself proves who is at each end), directly or
through a relay. The receiver accepts every file by hand. The rendezvous server
and the relay only introduce and carry; they never see file contents.

## 2. Assets

| Asset | Why it matters |
|---|---|
| **A1 Private key** | Whoever holds it *is* the device, to every peer that paired with it |
| **A2 `known_peers`** | The trust root: every "is this sender allowed" decision reads it |
| **A3 File contents and names** | The thing being sent |
| **A4 The receiver's disk** | Files already there (overwrite, path traversal), free space |
| **A5 The pairing code** | A six-digit secret whose job is to turn a Short ID into the right key |
| **A6 The receiver's attention** | Prompts are how a person consents; a spoofed or flooded prompt subverts consent |
| **A7 Presence and metadata** | When a device is online, its IP, who talks to whom, how much |
| **A8 Availability** | `listen`'s one transfer slot, memory, CPU, the rendezvous table |

## 3. Actors

| Actor | Can | Cannot (by assumption) |
|---|---|---|
| **N — network attacker** | See, drop, delay, replay, inject, reorder packets anywhere on the path, including between beam and the rendezvous server | Break TLS 1.3, Ed25519, SHA-256, SPAKE2 |
| **R — malicious rendezvous server** | Answer anything, forge entries, drop registrations, log everything it sees | Sign as a device; make iroh complete a handshake with the wrong key |
| **Y — malicious relay** (n0's or anyone's) | Everything N can, for relayed connections: see which keys talk, when, how much; drop or delay | Read or alter the QUIC stream it carries |
| **S — stranger who knows the Short ID** | Look the device up, connect to it, try pairing codes, grind a colliding Short ID | Know the current code, hold any paired key |
| **P — paired-but-malicious peer** | Everything a paired peer may do: send requests of any shape, go quiet, send malformed frames, choose file names and hints | Skip the receiver's Accept; impersonate *another* paired peer |

Out of scope: a compromised device (malware reading `~/.beam`), a person who
types `yes` to a fingerprint they did not check, and denial of service by
volume against the network itself.

## 4. Threats and mitigations

STRIDE per actor. **Test** names are Rust test functions; the file is given
where it is not obvious. "→ R-n" points to an accepted risk in section 5.

### N — network attacker

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Pretend to be a paired peer when sending | The receiver identifies the sender by the key the QUIC/TLS handshake proved, not by the request; a request claiming another key is refused unprompted | 0025, 0031 | `impersonation::a_known_public_key_without_its_secret_key_gets_nowhere`, `a_paired_device_claiming_another_ones_key_is_refused` (`tests/listen.rs`) |
| **S** | Pretend to be the receiver (answer a `send`) | `send` dials the endpoint id that *is* the stored key; iroh only completes a handshake with its holder; `remote_id()` is checked again | 0031 | `impersonation::a_rendezvous_that_returns_the_wrong_address_cannot_redirect_a_send` |
| **S** | Man-in-the-middle during pairing | SPAKE2 identities and the confirmation MAC cover both proved keys, the Short ID and the role; a relaying attacker's keys make the MACs fail on both sides | 0026 | `pairing::protocol::an_attacker_relaying_with_its_own_key_cannot_complete_pairing` (mutation-checked) |
| **T** | Alter file bytes in flight | QUIC/TLS integrity; per-chunk SHA-256 before write; whole-file SHA-256 before commit | 0015, 0025 | `the_receiver_refuses_a_chunk_that_fails_its_hash` (`tests/transfer.rs`), `a_file_that_fails_its_final_hash_is_discarded_and_never_written` (`tests/resume.rs`) |
| **T** | Alter a rendezvous registration or lookup answer | Registrations signed (domain-separated) by the device key; lookup answers re-checked by the client | 0027, 0031 | `rendezvous::proto::a_tampered_body_fails_the_signature`, `the_client_drops_lookup_entries_that_do_not_check_out`, `impersonation::a_rendezvous_answer_for_another_key_is_ignored` |
| **R** | Replay an old registration to restore a stale address | Timestamps within ±60 s and strictly newer per key | 0027 | `stale_and_future_timestamps_are_refused`, `an_old_registration_cannot_be_replayed_over_a_newer_one` |
| **R** | Replay a transfer | Random transfer ids; a replayed id is refused | 0015 | `a_replayed_transfer_id_is_refused` (`tests/transfer.rs`) |
| **I** | Read file contents or names | End-to-end QUIC/TLS | 0025 | Transport property (iroh); not re-proved by a beam test |
| **I** | Learn who looks up whom on the rendezvous connection | `wss://` in deployment | 0027, `deploy.md` | Verified manually through Cloudflare (`deploy.md`) → R-2 |
| **D** | Drop or delay traffic | Timeouts everywhere: accept 60 s, stall 60 s, QUIC idle 15 s; partials kept for resume | 0022, 0030, 0033 | `killing_the_sender_over_iroh_then_resuming`, `a_sender_that_goes_quiet_after_accept_is_given_up_on` |

### R — malicious rendezvous server

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Return the wrong address for a key, to redirect a `send` | iroh dials the key, not the address; the handshake fails, no data is sent | 0031 | `impersonation::a_rendezvous_that_returns_the_wrong_address_cannot_redirect_a_send` |
| **S** | Return a different key under a looked-up key or Short ID | The client drops entries whose key is not the one asked about, or does not derive the Short ID | 0027, 0031 | `impersonation::a_rendezvous_answer_for_another_key_is_ignored`, `the_client_drops_lookup_entries_that_do_not_check_out` |
| **S** | Insert its own device under a victim's Short ID during pairing | It still does not know the code: SPAKE2 fails; the real device is still returned; the fingerprint prompt stands behind both | 0026, 0027 | `colliding_short_ids_return_every_entry`, `a_wrong_code_pairs_nobody_and_uses_the_code_up` → R-4 |
| **T** | Forge a registration for someone else | It cannot sign; only signed registrations are stored | 0027 | `a_signature_by_another_key_fails` |
| **I** | Record presence, IPs, lookups | Nothing prevents a server from logging what it is told → R-2. beam's own server keeps memory only and logs nothing | 0027 | `beam-server` behaviour; not a test |
| **D** | Refuse service | Nothing prevents it; `listen` warns and retries, re-registers when the server returns | 0030 | Verified manually (`deploy.md`) |
| **E** | Put control sequences in an error message to drive the user's terminal | Server text reaches the screen only through `untrusted::text` | 0034 | `impersonation::a_rendezvous_error_cannot_carry_escape_sequences` |

### Y — malicious relay

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **T/S** | Alter or inject into a relayed connection | The relay carries QUIC it cannot decrypt or forge | 0025 | Transport property; `n0-data.md` |
| **I** | See which keys talk, when, and how much | Not prevented → R-3. `relay = "none"` or a self-hosted relay removes n0 from the picture | 0029 | `n0-data.md` |
| **D** | Drop relayed traffic | Direct paths when hole punching succeeds; timeouts and resume otherwise | 0032 | Manual (two networks, `test-plan.md`) |

### S — stranger who knows the Short ID

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Guess the pairing code | One guess per SPAKE2 run; single-use code; a failure pauses pairing (5 s, doubling); **three failures turn pairing off** for the `listen` session | 0026, 0028 | `a_wrong_code_pairs_nobody_and_uses_the_code_up`, `three_failed_attempts_switch_pairing_off_but_not_transfers`, `rotation::three_failures_in_a_row_turn_pairing_off_for_the_session` |
| **S** | Send a file without pairing | Refused on the proved key, unprompted | 0031 | `impersonation::an_unknown_key_is_refused_without_a_prompt`, `an_unpaired_sender_is_refused_without_a_prompt` (process level) |
| **S** | Grind a key with a colliding Short ID | Harmless for pairing (see R above) → R-4 | 0027 | `colliding_short_ids_return_every_entry` |
| **R** | Deny having tried | Every attempt is printed on the `listen` screen | 0028 | `listen_pairs_but_only_with_yes_in_full` (output) |
| **D** | Burn pairing codes by connecting | Accepted → R-1 | 0028 | `an_attempt_during_a_pause_is_refused_without_counting` |
| **E** | A crafted host-name hint to confuse the pairing prompt | Hints are reduced to `[A-Za-z0-9._-]`; the prompt renders every name through `untrusted::name` | 0030, 0034 | `a_hostile_hint_becomes_a_plain_name`, `the_pairing_prompt_cannot_be_driven_by_the_name` |

### P — paired-but-malicious peer

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Impersonate *another* paired peer | The proved key outranks the claim | 0031 | `a_paired_device_claiming_another_ones_key_is_refused` |
| **T** | Overwrite a file, escape the destination directory | Base-name only; separators, `..`, drive letters, ADS, reserved names refused; never overwrite (numbered instead) | 0017 | `traversal_attempts_are_reduced_to_a_base_name`, `colliding_names_are_numbered` (`transfer::paths`), `a_colliding_name_is_numbered_and_reported_to_both_sides` (`tests/transfer.rs`) |
| **T** | Attach to another peer's partial, or poison one | Partials matched by (fingerprint, file SHA-256, size, chunk size); have-bitmaps validated; chunks re-verified on reuse | 0021, 0024 | `a_corrupted_stored_chunk_is_re_fetched_rather_than_trusted` and the partial-matching tests in `tests/resume.rs`; `bitmap` unit tests |
| **E** | Send bytes before being accepted | Refused and discarded (S-4) | 0015 | `s3_the_sender_emits_no_file_bytes_before_accept`, `s4_data_before_accept_aborts_and_discards` (`tests/transfer.rs`), `data_before_accept_is_discarded` (over iroh, `tests/listen.rs`) |
| **E** | Skip the Accept prompt | No auto-accept anywhere; the flag allowlists prove it | 0015 | `listen_has_no_flag_that_could_stand_in_for_the_prompt`, `no_command_offers_an_auto_accept_switch` (`tests/cli.rs`), `s1_the_prompt_is_the_only_route_to_accepting` |
| **E** | **Drive the receiver's terminal** with a file name, cancel reason or error text | Control characters and whole ANSI sequences removed; bidi and zero-width characters shown as `<U+XXXX>`; names with bidi overrides refused; long names cut in the middle keeping the extension | 0034 | `tests/injection.rs`, `untrusted` unit tests |
| **S** | Disguise a file's type (`invoice‮fdp.exe`, a name long enough to hide `.exe`) | Bidi overrides/isolates refused in names; middle truncation | 0034 | `a_name_with_a_bidi_override_is_refused_before_the_prompt`, `a_long_name_shows_its_real_extension_in_the_prompt` |
| **D** | Ask for an absurd size | Plan checks, then free space, **before anything is written** | 0033 | `a_sixty_four_tib_request_is_refused_for_space_without_a_prompt` |
| **D** | Make the receiver allocate huge buffers | `MAX_CHUNK_SIZE` 16 MiB; frames ≤ 64 KiB checked from the header | 0016, 0033 | `a_chunk_size_over_the_cap_is_refused`, `an_oversized_frame_is_refused_from_its_header` |
| **D** | Make the receiver keep huge per-chunk state | `MAX_CHUNK_COUNT` 2²²; the `u32` count cannot wrap | 0033 | `a_request_with_too_many_chunks_is_refused`, `a_size_whose_chunk_count_would_wrap_is_refused` |
| **D** | Hold `listen`'s transfer slot | Stall timeout (60 s) between frames once accepted; the slot is freed as soon as the transfer ends | 0033 | `a_paired_peer_holding_the_slot_is_dropped_and_the_next_can_send` |
| **D** | Hold the sender | The same stall timeout on the sender; keep-alives only during verification | 0033 | `a_receiver_that_never_acknowledges_is_given_up_on`, `a_slow_final_verification_does_not_trip_the_stall_timeout` (+ its control) |
| **D** | Malformed frames | Strict decoding, unknown fields refused | 0016 | `malformed_frames_are_refused_without_a_prompt` |
| **D** | Flood the prompt | One transfer at a time (others: `Busy`); one question at a time, each with its own deadline; notices redraw the open question rather than hide it | 0030 | `a_second_transfer_while_one_is_open_is_told_to_try_later`, `cli::desk` tests |

### Key change (rule 3)

A peer that ran `beam init` again has a new key and a new Short ID, and nothing
links them to the old ones — deliberately: the address *is* the key, so there
is no identifier under which a changed key could be noticed. A changed key
therefore never gets in: it appears as **"not reachable"** to its senders and
as **"unknown key"** to its receivers, and both messages carry an SSH-style
warning to check the new fingerprint in person before re-pairing, because an
impersonator would ask for exactly that. Pairing again under the old name is
refused with the same warning. The stored key is never updated.
Tests: `a_receiver_that_re_ran_init_gets_a_re_pair_warning_not_a_transfer`,
`a_sender_that_re_ran_init_is_refused_with_a_re_pair_warning`,
`dialling_a_key_nobody_holds_any_more_finds_nobody` (`tests/listen.rs`).

## 5. Accepted risks

| | Risk | Why accepted | What limits it |
|---|---|---|---|
| **R-1** | **Code burning.** Anyone who can reach `listen` can spend its pairing code by connecting, and three failed attempts switch pairing off for the session. That is a denial of *pairing*. | Single use is what bounds guessing; making codes survive a failed attempt would give guesses back. | Transfers are unaffected; attempts are printed; restarting `listen` restores pairing; pairing is needed once per peer. |
| **R-2** | **Presence is visible to the rendezvous server** (and to Cloudflare behind a tunnel): which keys are online, their IPs, who looks up whom. | Someone has to introduce devices; the alternative, n0's discovery, publishes presence to a public server instead. | Memory only, no logs in `beam-server`; `wss://` hides it from the network; the server is yours to run. `n0-data.md`, `deploy.md`. |
| **R-3** | **Metadata is visible to the relay** (n0's by default): which keys talk, when, how much. Not contents. | Behind CGNAT a relay is the only path; n0's is free and in Asia-Pacific. | `relay = "none"`, or a self-hosted `iroh-relay`. `n0-data.md`. |
| **R-4** | **Short ID grinding.** ~2³⁰ key generations produce a colliding Short ID. | Pairing is not weakened (section 4, R and S). The Short ID is a routing hint, used once. | Filling a Short ID's eight slots (8 × 2³⁰ work) denies *pairing* with that device for as long as the attacker keeps all eight registrations alive — each needs an open connection refreshed every 30 s. Transfers, which look up the full key, are unaffected. |
| **R-5** | **The TCP test transport (`--addr`)** claims the sender's key without proving it and is unencrypted. | It exists so the engine can be tested without iroh. | Hidden; warns loudly on any non-loopback address; never used by `listen`/`send` without the flag. |
| **R-6** | **A crashed sender holds `listen`'s slot for up to 15 s** (QUIC idle timeout) and a live-but-silent one for up to 60 s (stall timeout). | Shorter would cut off real transfers on slow links or during verification. | Others get `Busy` in words and can retry. |
| **R-7** | **Notices can print while a question is open.** | A notice may matter (pairing turned off, a transfer refused). | The desk prints it and redraws the question; a notice never answers anything. |

## 6. Assumptions

- The cryptography in the libraries — Ed25519 (`ed25519-dalek`), TLS 1.3 with
  raw public keys (iroh/rustls), SPAKE2 (`spake2`), SHA-256, HMAC — is sound and
  correctly used by those libraries. beam invents none (rule 4).
- The operating system protects `~/.beam` from other local users (mode 0600 on
  Unix; ADR-0004 for Windows).
- People compare fingerprints when asked. The pairing prompt is designed to make
  that the natural thing to do, and to make `y` insufficient; it cannot make it
  certain.
