package main

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"syscall"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/creack/pty"
	"golang.org/x/term"
)

func runProxy(args []string) error {
	// args[0] is "opencode"
	cmd := exec.Command(args[0], args[1:]...)

	ptmx, err := pty.Start(cmd)
	if err != nil {
		return err
	}
	defer func() { _ = ptmx.Close() }()

	// Handle window size changes
	ch := make(chan os.Signal, 1)
	signal.Notify(ch, syscall.SIGWINCH)
	go func() {
		for range ch {
			if err := pty.InheritSize(os.Stdin, ptmx); err != nil {
				// ignore
			}
		}
	}()
	ch <- syscall.SIGWINCH
	defer func() { signal.Stop(ch); close(ch) }()

	oldState, err := term.MakeRaw(int(os.Stdin.Fd()))
	if err != nil {
		return err
	}
	defer func() { _ = term.Restore(int(os.Stdin.Fd()), oldState) }()

	// We print a nice header
	os.Stdout.Write([]byte("\r\n\033[36m[Better Review] Proxy active. Press \033[1mCtrl+O\033[0m\033[36m at any time to review uncommitted code changes.\033[0m\r\n\n"))

	// Copy stdout
	go func() {
		_, _ = io.Copy(os.Stdout, ptmx)
	}()

	// Handle exit
	go func() {
		_ = cmd.Wait()
		term.Restore(int(os.Stdin.Fd()), oldState)
		os.Exit(0)
	}()

	// Read stdin and intercept Ctrl+O
	buf := make([]byte, 1)
	for {
		n, err := os.Stdin.Read(buf)
		if err != nil {
			break
		}
		if n > 0 {
			if buf[0] == 0x0F { // Ctrl+O
				// Restore terminal to normal mode for the UI
				term.Restore(int(os.Stdin.Fd()), oldState)

				// Run the review
				_ = runReview()

				// Give the terminal back to raw mode for opencode
				oldState, _ = term.MakeRaw(int(os.Stdin.Fd()))

				// Optional: print a newline or just redraw prompt
				os.Stdout.Write([]byte("\r\n\033[36m[Better Review] Returned to opencode.\033[0m\r\n"))
				continue
			}
			_, _ = ptmx.Write(buf[:n])
		}
	}

	return nil
}

func runReview() error {
	cwd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("failed to get current working directory: %v", err)
	}

	diff, err := CollectGitDiff(context.Background(), cwd)
	if err != nil {
		return fmt.Errorf("error collecting git diff: %v", err)
	}

	if diff == "" {
		fmt.Println("\r\n\033[33m[Better Review] No uncommitted changes found.\033[0m")
		return nil
	}

	parsedFiles, err := ParseGitDiff(diff)
	if err != nil {
		return fmt.Errorf("error parsing git diff: %v", err)
	}

	// Use AltScreen to prevent messing up the opencode log
	p := tea.NewProgram(initialModel(parsedFiles), tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		return fmt.Errorf("error running review TUI: %v", err)
	}
	return nil
}
