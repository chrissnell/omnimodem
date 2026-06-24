package main

import (
	"flag"
	"fmt"
	"os"

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

func defaultSock() string {
	if dir := os.Getenv("XDG_RUNTIME_DIR"); dir != "" {
		return dir + "/omnimodem.sock"
	}
	return "/run/omnimodem.sock"
}
