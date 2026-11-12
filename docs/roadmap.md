# Roadmap

What comes next for beam. Finished work moves from here to the
[changelog](../CHANGELOG.md). Items name the requirement or decision they
belong to where one exists. What beam deliberately does not do is under
*Design boundaries* in [requirements.md](requirements.md#5-design-boundaries).

The current release is **0.1.0**.

## Now: 0.2.0

- **AUR packages.** Publish `beam` and `beam-gui` to the AUR from
  `packaging/arch/PKGBUILD` once the AUR takes new accounts again; its
  registration was temporarily closed when 0.1.0 was made (ADR-0058).
- **Why cross-network transfers stay on the relay.** A first test between two
  homes about 50 km apart moved a 5 GB file intact, but at about 1 MB/s,
  through the relay. Measure how often hole punching succeeds, and why it
  fails (for example behind mobile carrier-grade NAT), before changing
  anything.
- **Release workflow.** Tagging a version builds the Windows downloads in CI,
  the way `scripts/windows/package.ps1` builds them by hand now, with
  checksums, attaches them to the GitHub release, and updates the AUR
  package (ADR-0050, ADR-0056, ADR-0058).

## Next

- **Folder transfer:** send a folder as one transfer, with one Accept prompt
  and the same resume.
- **Bandwidth limit:** an optional cap for `send` and `listen`.
- **A scannable `listen` (R-8):** a running `listen` with router port mapping
  can be found by scanning. Planned rather than patched, because each fix
  costs usability or connectivity (ADR-0038).

## Later

- Sending several files in one request.
- A self-hosted relay in or near Thailand, closer than n0's public one
  (`iroh-relay`), and a guide to running your own.
- Code-signed Windows installers, so SmartScreen does not warn.
