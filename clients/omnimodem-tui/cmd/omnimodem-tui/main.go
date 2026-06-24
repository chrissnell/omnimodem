package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/app"
	"github.com/chrissnell/omnimodem/clients/omnimodem-tui/internal/client"
	tea "github.com/charmbracelet/bubbletea"
)

func main() {
	addr := flag.String("addr", defaultSock(), "omnimodemd address: a UDS path or host:port")
	flag.Parse()

	c, err := client.Dial(*addr)
	if err != nil {
		fmt.Fprintln(os.Stderr, "dial:", err)
		os.Exit(1)
	}
	defer c.Close()

	if _, err := tea.NewProgram(app.New(c, *addr), tea.WithAltScreen()).Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// defaultSock mirrors omnimodemd's own default so the two connect with no flags:
// OMNIMODEM_RUNTIME_DIR if set, else <tempdir>/omnimodem (Go's os.TempDir matches
// the daemon's std::env::temp_dir), with the socket inside it.
func defaultSock() string {
	dir := os.Getenv("OMNIMODEM_RUNTIME_DIR")
	if dir == "" {
		dir = filepath.Join(os.TempDir(), "omnimodem")
	}
	return filepath.Join(dir, "omnimodem.sock")
}
