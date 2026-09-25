# Deploying the rendezvous server

`beam-server` introduces devices to each other: it maps a Short ID or a public
key to an iroh endpoint address while a device is listening. It never sees a
file, a pairing code or a private key, and it keeps nothing on disk
(ADR-0027). This page runs it on the internet behind **`wss://`**, in one of
two ways, and points beam at it.

The **relay** is a separate thing and is not covered here: beam keeps n0's
Asia-Pacific relay as the default for now (`docs/n0-data.md`, ADR-0029).

## Why `wss://` and not plain `ws://`

What the server says does not depend on TLS for its *integrity*:
registrations are signed by the device key, and every lookup answer is checked
by the client (ADR-0027). A tampering network can deny service, but it cannot
make one device pair with another.

TLS is for *privacy*. Over `ws://`, anyone on the path sees which Short IDs and
keys are online and who looks up whom. Over `wss://`, only the server — and
whoever terminates TLS for it — sees that.

## Option A: a VPS behind a reverse proxy

Any small Linux VPS will do; the server is a single static binary and uses a
few megabytes of memory. You need a DNS name pointing at the machine
(`rv.example.org` below) and ports 80 and 443 open.

### 1. Build and install the server

On a machine with Rust 1.91 or newer (the VPS itself is fine):

```bash
cargo build --release -p beam-server
sudo install -m 0755 target/release/beam-server /usr/local/bin/beam-server
```

### 2. Run it as a service, on loopback only

The proxy is the only thing that should reach it.

`/etc/systemd/system/beam-server.service`:

```ini
[Unit]
Description=beam rendezvous server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/beam-server --addr 127.0.0.1:8787
Restart=on-failure
DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now beam-server
journalctl -u beam-server        # two lines at start-up, then nothing: it does not log requests
```

### 3a. Caddy (simplest)

Caddy obtains and renews the certificate itself, and proxies WebSocket
upgrades without extra configuration.

`/etc/caddy/Caddyfile`:

```
rv.example.org {
    reverse_proxy /v1 127.0.0.1:8787
}
```

```bash
sudo systemctl reload caddy
```

### 3b. nginx

With a certificate from certbot (`sudo certbot --nginx -d rv.example.org`):

```nginx
server {
    listen 443 ssl;
    server_name rv.example.org;

    ssl_certificate     /etc/letsencrypt/live/rv.example.org/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/rv.example.org/privkey.pem;

    # Nothing is served except the rendezvous endpoint.
    location = /v1 {
        proxy_pass http://127.0.0.1:8787;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        # A listening device refreshes every 30 s; the server itself closes a
        # connection after 120 s of silence. Keep the proxy out of the way.
        proxy_read_timeout 180s;
    }

    location / {
        return 404;
    }
}
```

`Upgrade` and `Connection` are the two lines that matter: without them nginx
answers the WebSocket handshake itself and every client reports the server as
unreachable.

**Access logs.** The proxy logs requests even though `beam-server` does not.
The request line is always `GET /v1`, so a log entry records only that some IP
connected, not which Short ID it asked about — but it is still a record of
which addresses use the service. Turn it off (`access_log off;` in the
`location`, or `log { output discard }` in Caddy) if you do not need it.

## Option B: Cloudflare Tunnel

No public IP, no open ports, no certificate to manage: `cloudflared` makes an
outbound connection to Cloudflare, and Cloudflare serves `wss://` for you. It
works from a home machine or a university lab machine behind NAT.

The trade-off, stated plainly: **Cloudflare terminates TLS**, so Cloudflare —
not only you — can see what the server sees: which keys and Short IDs are
online, their endpoint addresses, and who looks up whom. It still cannot forge
a registration or make two devices pair, for the same reasons as above.

Run `beam-server` on the machine as in step 2 of option A (or just
`beam-server` in a terminal).

### Quick tunnel — for trying it out

No Cloudflare account needed. The URL is random and changes every time.

```bash
cloudflared tunnel --url http://localhost:8787
```

It prints a line like:

```
https://bright-example-words.trycloudflare.com
```

The rendezvous URL is that host with `wss://` and `/v1`:

```
wss://bright-example-words.trycloudflare.com/v1
```

### Named tunnel — for keeping it

Needs a Cloudflare account and a domain on it.

```bash
cloudflared tunnel login
cloudflared tunnel create beam-rv
cloudflared tunnel route dns beam-rv rv.example.org
```

`~/.cloudflared/config.yml`:

```yaml
tunnel: <the tunnel UUID printed by `create`>
credentials-file: /home/<you>/.cloudflared/<the tunnel UUID>.json
ingress:
  - hostname: rv.example.org
    service: http://localhost:8787
  - service: http_status:404
```

```bash
cloudflared tunnel run beam-rv
# or, to start it at boot:
sudo cloudflared service install
```

Cloudflare proxies WebSockets on every plan; there is nothing to switch on.

## Pointing beam at it

On **every** device, in `~/.beam/config.toml` (create it if it does not
exist):

```toml
rendezvous = "wss://rv.example.org/v1"
```

Leave `relay` out to keep the default. Devices on different rendezvous servers
cannot find each other, so everyone who wants to pair or send has to use the
same one.

On Windows, `~/.beam` is `%USERPROFILE%\.beam`. PowerShell:

```powershell
'rendezvous = "wss://rv.example.org/v1"' | Set-Content "$env:USERPROFILE\.beam\config.toml" -Encoding utf8
```

(PowerShell 5.1 writes a byte-order mark with `-Encoding utf8`; beam ignores
it.)

## Checking it works

```bash
beam listen
```

It should show the Short ID and pairing code and **no** warning. If it cannot
reach the server it says so on stderr and keeps retrying:

```
beam: warning: cannot register with the rendezvous server (...); retrying.
```

The usual causes: a `ws://` URL for a server that only speaks `wss://` (or the
other way round), a missing `/v1`, or — behind nginx — missing `Upgrade` /
`Connection` headers.

From a second device, `beam pair <Short ID> --name <name>` then finds the
first one through the server.

**A brand-new quick-tunnel name takes a while to resolve** — up to a minute on
some networks, and the A (IPv4) record can lag behind the AAAA one. A lookup
made too early can be answered "no such name", and that answer is then cached.
If beam says `client DNS lookup failed` right after starting a quick tunnel,
wait a minute; on Windows, `ipconfig /flushdns` clears a cached failure. A named
tunnel or a VPS with a fixed DNS name does not have this problem.

## What was verified

On 2026-09-25, on the project's Windows 11 machine, with `cloudflared`
2026.9.3 (the binary signed by Cloudflare, Inc.) and the debug build of beam:

| Step | Result |
|---|---|
| `beam-server --addr 127.0.0.1:<port>` + `cloudflared tunnel --url http://127.0.0.1:<port>` | a `https://….trycloudflare.com` name; `wss://…/v1` answers the WebSocket upgrade with `101 Switching Protocols` |
| Two beam homes with `rendezvous = "wss://…/v1"`, relay left at the n0 default | `beam listen` registered and showed its Short ID after 0.7 s, with no warning |
| `beam pair <Short ID> --name bob`, code typed, `yes` on both sides | paired through the tunnel |
| `beam send bob payload.bin` (3 MiB) | found by key through the tunnel, `[Direct P2P]`, 0.9 s, SHA-256 identical |
| `beam-server` killed under a running `listen` | `listen` warned that it could not register |
| `beam-server` started again on the same port | `listen` printed "Registered with the rendezvous server again." |

The one surprise was DNS: the first lookups of the new name failed for about
fifty seconds (see the note above). Everything beam does over `wss://` — the
TLS handshake with webpki roots, the upgrade through Cloudflare, signed
registration and checked lookups — worked unchanged.

Option A (VPS + Caddy/nginx) was not run for this document: it needs a public
machine and a DNS name. The configurations above are the standard WebSocket
proxy setups for each server; the `Upgrade`/`Connection` headers are the part
that commonly goes wrong.
