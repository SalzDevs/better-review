package main

import (
	"fmt"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

var (
	addedStyle   = lipgloss.NewStyle().Foreground(lipgloss.Color("2"))   // Green
	removedStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("1"))   // Red
	headerStyle  = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("6")) // Cyan
	contextStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("244")) // Gray
	cursorStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("5")).Bold(true) // Magenta
)

type model struct {
	files      []FileDiff
	cursorFile int
}

func initialModel(files []FileDiff) model {
	return model{
		files:      files,
		cursorFile: 0,
	}
}

func (m model) Init() tea.Cmd {
	return nil
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "q":
			return m, tea.Quit
		case "up", "k":
			if m.cursorFile > 0 {
				m.cursorFile--
			}
		case "down", "j":
			if m.cursorFile < len(m.files)-1 {
				m.cursorFile++
			}
		}
	}
	return m, nil
}

func (m model) View() string {
	if len(m.files) == 0 {
		return "No changes found.\nPress q to quit."
	}

	var s strings.Builder

	s.WriteString(headerStyle.Render("Better Review - Agentic Code Review\n\n"))

	// Sidebar / File list
	for i, f := range m.files {
		cursor := "  "
		style := lipgloss.NewStyle()
		if m.cursorFile == i {
			cursor = "> "
			style = cursorStyle
		}
		s.WriteString(style.Render(fmt.Sprintf("%s%s\n", cursor, f.NewPath)))
	}

	s.WriteString("\n")

	// Diff view for current file
	currFile := m.files[m.cursorFile]
	s.WriteString(headerStyle.Render(fmt.Sprintf("--- a/%s\n+++ b/%s\n", currFile.OldPath, currFile.NewPath)))

	for _, hunk := range currFile.Hunks {
		s.WriteString(lipgloss.NewStyle().Foreground(lipgloss.Color("6")).Render(hunk.Header) + "\n")
		for _, line := range hunk.Lines {
			content := line.Content
			switch line.Kind {
			case "add":
				s.WriteString(addedStyle.Render("+" + content) + "\n")
			case "remove":
				s.WriteString(removedStyle.Render("-" + content) + "\n")
			default:
				s.WriteString(contextStyle.Render(" " + content) + "\n")
			}
		}
	}

	s.WriteString("\nPress j/k to navigate files, q to quit.\n")
	return s.String()
}
