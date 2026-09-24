package cli

import (
	"encoding/json"
	"fmt"
	"os"
	"time"

	"github.com/spf13/cobra"

	"beam/internal/identity"
	"beam/internal/ui"
)

// selfJSON is the --json shape of `beam whoami` and `beam init`.
type selfJSON struct {
	ShortID     string `json:"short_id"`
	Fingerprint string `json:"fingerprint"`
	KeyType     string `json:"key_type"`
	PublicKey   string `json:"public_key"`
	Comment     string `json:"comment,omitempty"`
	Dir         string `json:"dir"`
}

// peerJSON is the --json shape of one entry of `beam peers`.
type peerJSON struct {
	Name        string `json:"name"`
	ShortID     string `json:"short_id"`
	Fingerprint string `json:"fingerprint"`
	KeyType     string `json:"key_type"`
	PublicKey   string `json:"public_key"`
	Added       string `json:"added,omitempty"`
}

func (a *app) selfJSON(id *identity.Identity) selfJSON {
	return selfJSON{
		ShortID:     id.ShortID().String(),
		Fingerprint: id.Fingerprint().String(),
		KeyType:     identity.KeyType,
		PublicKey:   identity.EncodePublicKey(id.Public),
		Comment:     id.Comment,
		Dir:         a.store.Dir(),
	}
}

func writeJSON(cmd *cobra.Command, v any) error {
	enc := json.NewEncoder(cmd.OutOrStdout())
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

func newInitCommand(a *app) *cobra.Command {
	var force bool

	cmd := &cobra.Command{
		Use:   "init",
		Short: "Generate this device's identity keypair",
		Long: "init creates the Ed25519 keypair that identifies this device.\n\n" +
			"The private key never leaves this machine and is never sent to the\n" +
			"signaling server. Run this once per machine.",
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if a.store.HasIdentity() && !force {
				return fmt.Errorf("%w at %s\n       --force generates a new key, and every peer that paired with you would have to pair again",
					identity.ErrIdentityExists, a.store.PrivateKeyPath())
			}
			host, _ := os.Hostname()
			id, err := identity.Generate(host)
			if err != nil {
				return err
			}
			if err := a.store.SaveIdentity(id, force); err != nil {
				return err
			}

			out := cmd.OutOrStdout()
			if a.json {
				return writeJSON(cmd, a.selfJSON(id))
			}
			fmt.Fprintln(out, "Identity created.")
			fmt.Fprintln(out)
			ui.Field(out, "Short ID", id.ShortID().Display())
			ui.Field(out, "Fingerprint", id.Fingerprint().String())
			ui.Field(out, "Directory", a.store.Dir())
			fmt.Fprintln(out)
			fmt.Fprintln(out, "Give your Short ID to a peer so they can pair with you.")
			a.warnPermissions(cmd)
			return nil
		},
	}
	cmd.Flags().BoolVar(&force, "force", false, "replace an existing identity (invalidates every existing pairing)")
	return cmd
}

func newWhoamiCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "whoami",
		Short: "Show this device's Short ID and fingerprint",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			id, err := a.store.LoadIdentity()
			if err != nil {
				return err
			}
			out := cmd.OutOrStdout()
			if a.json {
				return writeJSON(cmd, a.selfJSON(id))
			}
			ui.Field(out, "Short ID", id.ShortID().Display())
			ui.Field(out, "Fingerprint", id.Fingerprint().String())
			ui.Field(out, "Public key", identity.KeyType+" "+identity.EncodePublicKey(id.Public))
			ui.Field(out, "Directory", a.store.Dir())
			a.warnPermissions(cmd)
			return nil
		},
	}
}

func newPeersCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "peers",
		Short: "List paired peers and their fingerprints",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			kp, err := a.store.LoadKnownPeers()
			if err != nil {
				return err
			}
			peers := kp.Peers()
			out := cmd.OutOrStdout()

			if a.json {
				list := make([]peerJSON, 0, len(peers))
				for _, p := range peers {
					entry := peerJSON{
						Name:        p.Name,
						ShortID:     p.ShortID().String(),
						Fingerprint: p.Fingerprint().String(),
						KeyType:     identity.KeyType,
						PublicKey:   identity.EncodePublicKey(p.PublicKey),
					}
					if !p.Added.IsZero() {
						entry.Added = p.Added.UTC().Format(time.RFC3339)
					}
					list = append(list, entry)
				}
				return writeJSON(cmd, list)
			}

			if len(peers) == 0 {
				fmt.Fprintln(out, "No paired peers yet. Use `beam pair <ID> --name <name>` to add one.")
				return nil
			}
			rows := make([][]string, 0, len(peers))
			for _, p := range peers {
				added := "-"
				if !p.Added.IsZero() {
					added = p.Added.UTC().Format("2006-01-02")
				}
				rows = append(rows, []string{p.Name, p.ShortID().Display(), p.Fingerprint().Short(), added})
			}
			if err := ui.Table(out, []string{"NAME", "SHORT ID", "FINGERPRINT", "ADDED"}, rows); err != nil {
				return err
			}
			a.warnPermissions(cmd)
			return nil
		},
	}
}

func newRenameCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "rename <old-name> <new-name>",
		Short: "Change the local nickname of a paired peer",
		Long: "rename changes only the local nickname. The peer's key is untouched,\n" +
			"and the peer is not notified: names are local labels, not identities.",
		Args: cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			oldName, newName := args[0], args[1]
			kp, err := a.store.LoadKnownPeers()
			if err != nil {
				return err
			}
			if err := kp.Rename(oldName, newName); err != nil {
				return err
			}
			if err := a.store.SaveKnownPeers(kp); err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "Renamed %s to %s.\n", oldName, newName)
			return nil
		},
	}
}

func newRemoveCommand(a *app) *cobra.Command {
	var assumeYes bool

	cmd := &cobra.Command{
		Use:   "remove <name>",
		Short: "Forget a paired peer",
		Long: "remove deletes the peer from known_peers. That peer can no longer send\n" +
			"you files, and pairing with it again requires a fresh pairing code.",
		Args: cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			name := args[0]
			kp, err := a.store.LoadKnownPeers()
			if err != nil {
				return err
			}
			peer, found := kp.Lookup(name)
			if !found {
				return fmt.Errorf("%w: %s", identity.ErrPeerNotFound, name)
			}
			if !assumeYes {
				out := cmd.OutOrStdout()
				ui.Field(out, "Name", peer.Name)
				ui.Field(out, "Fingerprint", peer.Fingerprint().String())
				ok, err := ui.Confirm(cmd.InOrStdin(), out, fmt.Sprintf("Remove %s?", peer.Name))
				if err != nil {
					return err
				}
				if !ok {
					fmt.Fprintln(out, "Cancelled.")
					return nil
				}
			}
			if err := kp.Remove(peer.Name); err != nil {
				return err
			}
			if err := a.store.SaveKnownPeers(kp); err != nil {
				return err
			}
			fmt.Fprintf(cmd.OutOrStdout(), "Removed %s.\n", peer.Name)
			return nil
		},
	}
	cmd.Flags().BoolVarP(&assumeYes, "yes", "y", false, "do not ask for confirmation")
	return cmd
}
