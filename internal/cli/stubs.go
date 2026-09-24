package cli

import (
	"fmt"
	"runtime/debug"

	"github.com/spf13/cobra"
)

// The commands below are declared now so that `beam --help` shows the whole
// intended surface from M0 on. Each one fails loudly with exit code 2 until
// its milestone lands, rather than pretending to work.

func newPairCommand(a *app) *cobra.Command {
	var name string

	cmd := &cobra.Command{
		Use:   "pair <short-id>",
		Short: "Pair with a peer for the first time using its Short ID and pairing code",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			return notImplemented("pair", "M4")
		},
	}
	cmd.Flags().StringVar(&name, "name", "", "local nickname to store the peer under")
	return cmd
}

func newListenCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "listen",
		Short: "Wait for incoming transfers and show the pairing code",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			return notImplemented("listen", "M2")
		},
	}
}

func newSendCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "send <peer> <file>",
		Short: "Send a file to a paired peer",
		Args:  cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			return notImplemented("send", "M2")
		},
	}
}

func newNewcodeCommand(a *app) *cobra.Command {
	return &cobra.Command{
		Use:   "newcode",
		Short: "Regenerate this device's pairing code",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			return notImplemented("newcode", "M4")
		},
	}
}

// version is overridden at build time with -ldflags "-X beam/internal/cli.version=..."
var version = "dev"

func newVersionCommand() *cobra.Command {
	return &cobra.Command{
		Use:   "version",
		Short: "Show the beam version",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			rev := ""
			if info, ok := debug.ReadBuildInfo(); ok {
				for _, s := range info.Settings {
					if s.Key == "vcs.revision" && len(s.Value) >= 7 {
						rev = " (" + s.Value[:7] + ")"
					}
				}
			}
			fmt.Fprintf(cmd.OutOrStdout(), "beam %s%s\n", version, rev)
			return nil
		},
	}
}
