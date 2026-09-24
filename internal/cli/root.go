// Package cli defines the beam command tree. Commands live here rather than in
// package main so they can be exercised by unit tests; see ADR-0002.
package cli

import (
	"errors"
	"fmt"
	"io"

	"github.com/spf13/cobra"

	"beam/internal/identity"
)

// Process exit codes.
const (
	ExitOK             = 0
	ExitError          = 1
	ExitNotImplemented = 2
)

// app carries the state shared by every command: where the beam home directory
// is and whether output should be machine readable.
type app struct {
	beamDir string
	json    bool
	store   *identity.Store
}

// notImplementedError marks a command that is stubbed until a later milestone.
type notImplementedError struct {
	command   string
	milestone string
}

func (e *notImplementedError) Error() string {
	return fmt.Sprintf("`beam %s` is not implemented yet (planned for milestone %s)", e.command, e.milestone)
}

func notImplemented(command, milestone string) error {
	return &notImplementedError{command: command, milestone: milestone}
}

// NewRootCommand builds the beam command tree.
func NewRootCommand() *cobra.Command {
	a := &app{}

	root := &cobra.Command{
		Use:   "beam",
		Short: "Identity-based peer-to-peer file transfer",
		Long: "beam sends files directly between two computers.\n\n" +
			"A peer must be paired before it can send you anything, and every incoming\n" +
			"transfer has to be accepted by hand. There is no auto-accept.",
		SilenceUsage:  true,
		SilenceErrors: true,
		PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
			dir := a.beamDir
			if dir == "" {
				var err error
				if dir, err = identity.DefaultDir(); err != nil {
					return err
				}
			}
			a.store = identity.NewStore(dir)
			return nil
		},
		RunE: func(cmd *cobra.Command, args []string) error {
			return cmd.Help()
		},
	}

	root.PersistentFlags().StringVar(&a.beamDir, "beam-dir", "",
		"beam home directory (default $BEAM_DIR, else ~/.beam)")
	root.PersistentFlags().BoolVar(&a.json, "json", false, "machine-readable JSON output")

	root.AddCommand(
		newInitCommand(a),
		newWhoamiCommand(a),
		newPeersCommand(a),
		newRenameCommand(a),
		newRemoveCommand(a),
		newPairCommand(a),
		newListenCommand(a),
		newSendCommand(a),
		newNewcodeCommand(a),
		newVersionCommand(),
	)
	return root
}

// Execute runs the command tree and returns the process exit code.
func Execute(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	root := NewRootCommand()
	root.SetArgs(args)
	root.SetIn(stdin)
	root.SetOut(stdout)
	root.SetErr(stderr)

	if err := root.Execute(); err != nil {
		fmt.Fprintf(stderr, "beam: %v\n", err)
		var ni *notImplementedError
		if errors.As(err, &ni) {
			return ExitNotImplemented
		}
		return ExitError
	}
	return ExitOK
}

// warnPermissions prints any file-permission warnings to stderr.
func (a *app) warnPermissions(cmd *cobra.Command) {
	for _, w := range a.store.PermissionWarnings() {
		fmt.Fprintf(cmd.ErrOrStderr(), "beam: warning: %s\n", w)
	}
}
