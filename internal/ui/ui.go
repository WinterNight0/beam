// Package ui holds terminal output helpers shared by the beam commands.
package ui

import (
	"bufio"
	"fmt"
	"io"
	"strings"
	"text/tabwriter"
)

// Table writes a simple aligned table. An empty rows slice prints headers only.
func Table(w io.Writer, headers []string, rows [][]string) error {
	tw := tabwriter.NewWriter(w, 0, 0, 2, ' ', 0)
	if _, err := fmt.Fprintln(tw, strings.Join(headers, "\t")); err != nil {
		return err
	}
	for _, row := range rows {
		if _, err := fmt.Fprintln(tw, strings.Join(row, "\t")); err != nil {
			return err
		}
	}
	return tw.Flush()
}

// Field writes one "label  value" line of a detail block.
func Field(w io.Writer, label, value string) {
	fmt.Fprintf(w, "  %-13s %s\n", label, value)
}

// Confirm asks a yes/no question. Anything other than "y" or "yes" is a no,
// and end-of-input is a no: a destructive action never proceeds by default.
func Confirm(in io.Reader, out io.Writer, question string) (bool, error) {
	fmt.Fprintf(out, "%s [y/N]: ", question)
	line, err := bufio.NewReader(in).ReadString('\n')
	if err != nil && line == "" {
		if err == io.EOF {
			fmt.Fprintln(out)
			return false, nil
		}
		return false, err
	}
	answer := strings.ToLower(strings.TrimSpace(line))
	return answer == "y" || answer == "yes", nil
}
