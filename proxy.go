package main

import (
	"context"
	"fmt"
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

	// Copy stdout manually with a 4096 byte buffer for snappy rendering
	go func() {
		bufOut := make([]byte, 4096)
		for {
			n, err := ptmx.Read(bufOut)
			if n > 0 {
				_, _ = os.Stdout.Write(bufOut[:n])
			}
			if err != nil {
				break
			}
		}
	}()

	// Handle exit
	go func() {
		_ = cmd.Wait()
		term.Restore(int(os.Stdin.Fd()), oldState)
		os.Exit(0)
	}()

	// Read stdin and intercept Ctrl+O
	buf := make([]byte, 4096)
	for {
		n, err := os.Stdin.Read(buf)
		if err != nil {
			break
		}
		if n > 0 {
			// Look for Ctrl+O (0x0F) in the buffer
			ctrlOIndex := -1
			for i := 0; i < n; i++ {
				if buf[i] == 0x0F {
					ctrlOIndex = i
					break
				}
			}

			if ctrlOIndex != -1 {
				// Write anything before Ctrl+O
				if ctrlOIndex > 0 {
					_, _ = ptmx.Write(buf[:ctrlOIndex])
				}

				// Run the review inline. Bubble Tea (in runReview) will handle
				// its own terminal state and AltScreen properly.
				_ = runReview()

				// Write anything after Ctrl+O
				if ctrlOIndex+1 < n {
					_, _ = ptmx.Write(buf[ctrlOIndex+1 : n])
				}
				continue
			}

			// No Ctrl+O, write everything
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
