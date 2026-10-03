# Threat model

What beam protects, from whom, how, and what it deliberately does not. Every
mitigation below names the decision behind it (ADR, in `decisions.md`) and the
test that demonstrates it; a claim without a test is marked as such.

Written for M6, against the code at that milestone: iroh `=1.2.0` as the
transport and n0's relay as the default relay. **Revised for ADR-0036
(merged after M6):** there is no rendezvous server; devices meet through an
invite and are found again by key through the relay. The rows about the
rendezvous server are replaced by section "I — whoever can alter an invite or
a saved address".

## 1. System in one paragraph

Two devices each hold an Ed25519 key (`~/.beam/id_ed25519`). They pair once:
one runs `beam listen`, the other `beam pair <invite>`; SPAKE2 over an iroh
connection proves both know a six-digit code, and each person confirms the
other's fingerprint with `yes`. Afterwards each device keeps the other's public
key in `known_peers`, next to the relay and addresses its invite named.
`beam send` dials the peer by that key, through its relay or at those addresses,
over iroh (QUIC/TLS, keyed by the same Ed25519 identities — so the connection
itself proves who is at each end), directly or through the relay. The receiver
accepts every file by hand. The relay only carries; it never sees file
contents.

## 2. Assets

| Asset | Why it matters |
|---|---|
| **A1 Private key** | Whoever holds it *is* the device, to every peer that paired with it |
| **A2 `known_peers`** | The trust root: every "is this sender allowed" decision reads it |
| **A3 File contents and names** | The thing being sent |
| **A4 The receiver's disk** | Files already there (overwrite, path traversal), free space |
| **A5 The pairing code** | A six-digit secret whose job is to turn an invite into the right key |
| **A6 The receiver's attention** | Prompts are how a person consents; a spoofed or flooded prompt subverts consent |
| **A7 Presence and metadata** | When a device is online, its IP, who talks to whom, how much |
| **A8 Availability** | `listen`'s one transfer slot, memory, CPU |

## 3. Actors

| Actor | Can | Cannot (by assumption) |
|---|---|---|
| **N — network attacker** | See, drop, delay, replay, inject, reorder packets anywhere on the path | Break TLS 1.3, Ed25519, SHA-256, SPAKE2 |
| **I — whoever can alter an invite or a saved address** | Rewrite an invite on its way to the joiner (swap the key, the relay, the addresses); hand someone a forged invite for a device they already paired; edit `addrs=`/`relay=` | Make iroh complete a handshake with the wrong key; know the code |
| **Y — malicious relay** (n0's or anyone's) | Everything N can, for relayed connections: see which keys talk, when, how much; drop or delay | Read or alter the QUIC stream it carries |
| **S — stranger who has the invite** | Connect to the device, try pairing codes | Know the current code, hold any paired key |
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
| **S** | Pretend to be the receiver (answer a `send`) | `send` dials the endpoint id that *is* the stored key; iroh only completes a handshake with its holder; `remote_id()` is checked again | 0031 | `impersonation::a_wrong_address_for_a_paired_key_cannot_redirect_a_send` |
| **S** | Man-in-the-middle during pairing | SPAKE2 identities and the confirmation MAC cover both proved keys, the Short ID and the role; a relaying attacker's keys make the MACs fail on both sides | 0026 | `pairing::protocol::an_attacker_relaying_with_its_own_key_cannot_complete_pairing` (mutation-checked) |
| **T** | Alter file bytes in flight | QUIC/TLS integrity; per-chunk SHA-256 before write; whole-file SHA-256 before commit | 0015, 0025 | `the_receiver_refuses_a_chunk_that_fails_its_hash` (`tests/transfer.rs`), `a_file_that_fails_its_final_hash_is_discarded_and_never_written` (`tests/resume.rs`) |
| **R** | Replay a transfer | Random transfer ids; a replayed id is refused | 0015 | `a_replayed_transfer_id_is_refused` (`tests/transfer.rs`) |
| **I** | Read file contents or names | End-to-end QUIC/TLS | 0025 | Transport property (iroh); not re-proved by a beam test |
| **D** | Drop or delay traffic | Timeouts everywhere: accept 60 s, stall 60 s, QUIC idle 15 s; partials kept for resume | 0022, 0030, 0033 | `killing_the_sender_over_iroh_then_resuming`, `a_sender_that_goes_quiet_after_accept_is_given_up_on` |

### I — whoever can alter an invite or a saved address

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Swap the key in an invite for the attacker's, keeping the real address | The joiner dials the swapped key; the real device cannot prove it, so the handshake fails before the protocol; the waiter's code is not spent | 0036 | `an_invite_with_a_swapped_key_reaches_nobody_and_spends_nothing` (`tests/pairing.rs`) |
| **S** | Swap the key *and* run the endpoint at the new address | The attacker still has to know the code (SPAKE2), and both people see and confirm fingerprints | 0026, 0036 | `pairing::protocol::an_attacker_relaying_with_its_own_key_cannot_complete_pairing`, `a_wrong_code_pairs_nobody_and_uses_the_code_up` → R-2 |
| **S** | Point a paired key at the attacker's address (a forged invite, a hand-edited `addrs=`) to redirect a `send` | iroh dials the key, not the address; the handshake fails, no data is sent | 0031, 0036 | `impersonation::a_wrong_address_for_a_paired_key_cannot_redirect_a_send` |
| **I** | A forged invite for a paired device that names the attacker's relay, to watch (when, from where) or block sends to that peer (F-1) | A relay change is saved only after a yes to a question that shows both relays and explains the risk; address-only updates cannot leak, because the relay still reaches the peer | 0038 | `tests/cli.rs::an_invite_that_changes_a_peers_relay_needs_a_yes` |
| **S** | An invite naming a plain-`http://` or local relay, to make beam connect to local services or talk to a relay in plain text (F-3) | Relays from invites and saved `relay=` must be `https://` on a public host; anything else is dropped, and the joiner uses its own relay | 0038 | `invite::an_http_or_local_relay_in_an_invite_is_dropped_not_used` |
| **T** | Use the address-update path (`beam pair <invite>` for a known device) to change a stored key | The update only writes `relay=`/`addrs=`, and a relay change needs a yes; the key is never replaced, and an invite for a new key is a new pairing | 0036, 0038 | `pairing_again_with_a_known_devices_invite_only_updates_its_address`, `an_invite_that_changes_a_peers_relay_needs_a_yes` (`tests/cli.rs`), `attributes_can_be_set_replaced_and_removed_and_round_trip` |
| **D** | Make a paired peer unreachable with a forged address-only update | Not prevented; it lasts until the next real invite → R-4 | 0036 | — |
| **T** | Tamper with CI or a dependency (supply chain) | CI token is read-only; actions pinned to commit hashes; `cargo audit` on every push; `iroh` and `spake2` pinned exactly | 0038 | `.github/workflows/ci.yml`; manual check recorded in `SECURITY.md` §7 |
| **E** | Put control sequences in an invite to drive the terminal | Invite errors never quote the input; error lines go through `untrusted::lines` | 0034, 0036 | `invite::what_is_not_an_invite_is_refused_without_quoting_it` |

### Y — malicious relay

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **T/S** | Alter or inject into a relayed connection | The relay carries QUIC it cannot decrypt or forge | 0025 | Transport property; `n0-data.md` |
| **I** | See which keys talk, when, and how much | Not prevented → R-3. `relay = "none"` or a self-hosted relay removes n0 from the picture | 0029 | `n0-data.md` |
| **D** | Drop relayed traffic | Direct paths when hole punching succeeds; timeouts and resume otherwise | 0032 | Manual (two networks, `test-plan.md`) |

### S — stranger who has the invite

| | Threat | Mitigation | ADR | Test |
|---|---|---|---|---|
| **S** | Guess the pairing code | One guess per SPAKE2 run; single-use code; a failure pauses pairing (5 s, doubling); **three failures turn pairing off** for the `listen` session | 0026, 0028 | `a_wrong_code_pairs_nobody_and_uses_the_code_up`, `three_failed_attempts_switch_pairing_off_but_not_transfers`, `rotation::three_failures_in_a_row_turn_pairing_off_for_the_session` |
| **S** | Send a file without pairing | Refused on the proved key, unprompted | 0031 | `impersonation::an_unknown_key_is_refused_without_a_prompt`, `an_unpaired_sender_is_refused_without_a_prompt` (process level) |
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
| **T** | Send a chunk twice, or one outside the transfer, to overwrite or confuse what was received | Only chunks still missing are accepted; anything else ends the transfer. The whole-file hash is checked regardless | 0040 | `a_duplicate_or_unknown_chunk_is_refused` (`tests/transfer.rs`) |
| **T** | The file changes under the sender (edited mid-send, or a disk returning bad data), so different bytes are sent than were promised | The sender checks every chunk against the hash taken before the request, and stops with CANCEL on a mismatch; the receiver's whole-file check is the backstop | 0040 | `a_file_changed_while_being_sent_is_caught_before_it_is_sent` (`tests/transfer.rs`) |
| **D** | Open many QUIC streams so the receiver buffers a stream window for each | The QUIC connection receive window is capped at 32 MiB across all streams (noq sets no cap by default) | 0039 | configuration in `transport::endpoint::transport_config` |
| **D** | Make the receiver keep huge per-chunk state | `MAX_CHUNK_COUNT` 2²²; the `u32` count cannot wrap | 0033 | `a_request_with_too_many_chunks_is_refused`, `a_size_whose_chunk_count_would_wrap_is_refused` |
| **D** | Hold `listen`'s transfer slot | Stall timeout (60 s) between frames once accepted; the slot is freed as soon as the transfer ends | 0033 | `a_paired_peer_holding_the_slot_is_dropped_and_the_next_can_send` |
| **D** | Hold the sender | The same stall timeout on the sender; keep-alives only during verification | 0033 | `a_receiver_that_never_acknowledges_is_given_up_on`, `a_slow_final_verification_does_not_trip_the_stall_timeout` (+ its control) |
| **D** | Malformed frames | Strict decoding, unknown fields refused | 0016 | `malformed_frames_are_refused_without_a_prompt` |
| **D** | Flood the prompt | One transfer at a time (others: `Busy`); one question at a time, each with its own deadline; notices redraw the open question rather than hide it | 0030 | `a_second_transfer_while_one_is_open_is_told_to_try_later`, `cli::desk` tests |

### Key change (rule 3)

A peer that ran `beam init` again has a new key and a new invite, and nothing
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
| **R-2** | **The invite and the code may travel together.** Someone who can read *and* rewrite that channel can put their own key in the invite and use the code. | Asking people to use two channels makes beam harder to use than the threat warrants. | Both screens show both fingerprints, and pairing needs `yes` typed in full on both; the attacker's fingerprint is not the real device's. |
| **R-3** | **Metadata is visible to the relay** (n0's by default): which keys talk, when, how much. Not contents. | Behind CGNAT a relay is the only path; n0's is free and in Asia-Pacific. | `relay = "none"`, or a self-hosted `iroh-relay`. `n0-data.md`. |
| **R-4** | **A forged address-only update** makes a paired peer unreachable. | Updates must not need a code, or a peer that moved networks could not be found again without re-pairing. | It cannot change the key or redirect a send, and since ADR-0038 a *relay* change needs a yes, so it cannot route sends through someone else's relay. The next real invite repairs it, and the relay still reaches the peer. |
| **R-5** | **The TCP test transport (`--addr`)** claims the sender's key without proving it and is unencrypted. | It exists so the engine can be tested without iroh. | Hidden; warns loudly on any non-loopback address; never used by `listen`/`send` without the flag. |
| **R-6** | **A crashed sender holds `listen`'s slot for up to 15 s** (QUIC idle timeout) and a live-but-silent one for up to 60 s (stall timeout). | Shorter would cut off real transfers on slow links or during verification. | Others get `Busy` in words and can retry. |
| **R-7** | **Notices can print while a question is open.** | A notice may matter (pairing turned off, a transfer refused). | The desk prints it and redraws the question; a notice never answers anything. |
| **R-8** | **Open (F-2).** `listen`'s fixed port plus router port mapping make a running `listen` findable by scanning; anyone can then use up its pairing codes (R-1, easier) and learn its public key. | Both fixes found trade away something users feel: disabling port mapping reduces direct connections (and breaks no-relay use across the internet); an invite secret checked before the code is spent changes the invite and handshake. To be planned (ADR-0038). | Transfers from unknown keys are refused unprompted; the code is single use and three failures switch pairing off; `listen` need only run while it is needed. |

## 6. Assumptions

- The cryptography in the libraries — Ed25519 (`ed25519-dalek`), TLS 1.3 with
  raw public keys (iroh/rustls), SPAKE2 (`spake2`), SHA-256, HMAC — is sound and
  correctly used by those libraries. beam invents none (rule 4).
- The operating system protects `~/.beam` from other local users (mode 0600 on
  Unix; ADR-0004 for Windows).
- People compare fingerprints when asked. The pairing prompt is designed to make
  that the natural thing to do, and to make `y` insufficient; it cannot make it
  certain.
