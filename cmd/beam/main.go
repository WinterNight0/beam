// Command beam is the identity-based peer-to-peer file transfer CLI.
package main

import (
	"os"

	"beam/internal/cli"
)

func main() {
	os.Exit(cli.Execute(os.Args[1:], os.Stdin, os.Stdout, os.Stderr))
}
