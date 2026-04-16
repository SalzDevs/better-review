package main

import (
	"fmt"
	"strings"

	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

var (
	addedStyle         = lipgloss.NewStyle().Foreground(lipgloss.Color("#59c9a5"))
	addedPrefixStyle   = lipgloss.NewStyle().Foreground(lipgloss.Color("#3db58f"))
	removedStyle       = lipgloss.NewStyle().Foreground(lipgloss.Color("#ef6f6c"))
	removedPrefixStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#da5a58"))
	headerStyle        = lipgloss.NewStyle().Foreground(lipgloss.Color("#f3f5f7")).PaddingLeft(2).PaddingRight(2).Background(lipgloss.Color("#26323b"))
	headerActiveStyle  = lipgloss.NewStyle().Foreground(lipgloss.Color("#101418")).PaddingLeft(2).PaddingRight(2).Background(accentColor).Bold(true)
	contextStyle       = lipgloss.NewStyle().Foreground(lipgloss.Color("#93a6b5"))
	contextPrefixStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#4a5a66"))

	gutterStyle       = lipgloss.NewStyle().Foreground(lipgloss.Color("#607282")).PaddingRight(1)
	gutterAddStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("#3db58f")).PaddingRight(1)
	gutterRemoveStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#da5a58")).PaddingRight(1)

	badgeAddStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#59c9a5")).Bold(true)
	badgeModStyle = lipgloss.NewStyle().Foreground(accentColor).Bold(true)
	badgeDelStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#ef6f6c")).Bold(true)

	badgeAccStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#59c9a5")).Bold(true)
	badgeRejStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("#ef6f6c")).Bold(true)

	activeItemStyle = lipgloss.NewStyle().
			Background(lipgloss.Color("#202a31")).
			Foreground(lipgloss.Color("#edf2f7")).
			Bold(true).
			Width(30).
			MaxWidth(30).
			PaddingLeft(1)

	inactiveItemStyle = lipgloss.NewStyle().
				Foreground(lipgloss.Color("#93a6b5")).
				Width(30).
				MaxWidth(30).
				PaddingLeft(1)

	sidebarStyleActive = lipgloss.NewStyle().
				Border(lipgloss.NormalBorder(), false, true, false, false).
				BorderForeground(accentColor).
				PaddingRight(1).
				MarginRight(1)

	sidebarStyleInactive = lipgloss.NewStyle().
				Border(lipgloss.NormalBorder(), false, true, false, false).
				BorderForeground(lipgloss.Color("#30404c")).
				PaddingRight(1).
				MarginRight(1)
)

type focusState int

const (
	focusSidebar focusState = iota
	focusViewport
)

type reviewModel struct {
	files      []FileDiff
	cursorFile int
	cursorHunk int
	ready      bool
	viewport   viewport.Model
	width      int
	height     int
	focus      focusState
	status     string
}

func newReviewModel(files []FileDiff) reviewModel {
	return reviewModel{
		files:      files,
		cursorFile: 0,
		cursorHunk: 0,
		focus:      focusSidebar,
		status:     "Review the generated changes.",
	}
}

func (m reviewModel) Init() tea.Cmd {
	return nil
}

func (m *reviewModel) hasFiles() bool {
	return len(m.files) > 0
}

func (m *reviewModel) inDiffView() bool {
	return m.focus == focusViewport
}

func (m *reviewModel) resize(width, height int) {
	m.width = width
	m.height = height
	footerHeight := lipgloss.Height("\nEsc: back | Enter: open diff | y: accept | x: reject | u: undo | Ctrl+C: quit")
	verticalMarginHeight := footerHeight + 5
	viewportWidth := width - 35
	if viewportWidth < 20 {
		viewportWidth = 20
	}
	viewportHeight := height - verticalMarginHeight
	if viewportHeight < 6 {
		viewportHeight = 6
	}

	if !m.ready {
		m.viewport = viewport.New(viewportWidth, viewportHeight)
		m.ready = true
	} else {
		m.viewport.Width = viewportWidth
		m.viewport.Height = viewportHeight
	}
	m.viewport.SetContent(m.renderDiff())
}

func (m *reviewModel) renderDiff() string {
	if len(m.files) == 0 {
		return "No changes."
	}

	var s strings.Builder
	currFile := m.files[m.cursorFile]

	for hIndex, hunk := range currFile.Hunks {
		hStyle := headerStyle
		if m.focus == focusViewport && m.cursorHunk == hIndex {
			hStyle = headerActiveStyle
		}

		status := ""
		if hunk.ReviewStatus == StatusAccepted {
			status = "  [Accepted]"
			hStyle = hStyle.Copy().Foreground(successColor)
		} else if hunk.ReviewStatus == StatusRejected {
			status = "  [Rejected]"
			hStyle = hStyle.Copy().Foreground(dangerColor)
		}

		s.WriteString("\n" + hStyle.Render(hunk.Header+status) + "\n")
		for _, line := range hunk.Lines {
			oldLineStr := "    "
			if line.OldLine > 0 {
				oldLineStr = fmt.Sprintf("%4d", line.OldLine)
			}

			newLineStr := "    "
			if line.NewLine > 0 {
				newLineStr = fmt.Sprintf("%4d", line.NewLine)
			}

			content := line.Content
			switch line.Kind {
			case "add":
				s.WriteString(gutterStyle.Render(oldLineStr) + gutterAddStyle.Render(newLineStr) + addedPrefixStyle.Render("+ ") + addedStyle.Render(content) + "\n")
			case "remove":
				s.WriteString(gutterRemoveStyle.Render(oldLineStr) + gutterStyle.Render(newLineStr) + removedPrefixStyle.Render("- ") + removedStyle.Render(content) + "\n")
			default:
				s.WriteString(gutterStyle.Render(oldLineStr) + gutterStyle.Render(newLineStr) + contextPrefixStyle.Render("  ") + contextStyle.Render(content) + "\n")
			}
		}
	}

	return s.String()
}

func (m reviewModel) Update(msg tea.Msg) (reviewModel, tea.Cmd) {
	var cmd tea.Cmd

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.resize(msg.Width, msg.Height)

	case tea.KeyMsg:
		switch msg.String() {
		case "enter":
			if m.focus == focusSidebar {
				m.focus = focusViewport
				m.cursorHunk = 0
				m.viewport.SetContent(m.renderDiff())
				m.status = "Inspecting hunks."
			}

		case "esc":
			if m.focus == focusViewport {
				m.focus = focusSidebar
				m.cursorHunk = 0
				m.viewport.SetContent(m.renderDiff())
				m.status = "Reviewing files."
			}

		case "tab":
			if m.focus == focusViewport {
				f := &m.files[m.cursorFile]
				if len(f.Hunks) > 0 {
					m.cursorHunk = (m.cursorHunk + 1) % len(f.Hunks)
					m.viewport.SetContent(m.renderDiff())
				}
			}

		case "shift+tab":
			if m.focus == focusViewport {
				f := &m.files[m.cursorFile]
				if len(f.Hunks) > 0 {
					m.cursorHunk--
					if m.cursorHunk < 0 {
						m.cursorHunk = len(f.Hunks) - 1
					}
					m.viewport.SetContent(m.renderDiff())
				}
			}

		case "up", "k":
			if m.focus == focusSidebar {
				if m.cursorFile > 0 {
					m.cursorFile--
					m.viewport.SetContent(m.renderDiff())
					m.viewport.GotoTop()
				}
			} else {
				m.viewport.LineUp(1)
			}

		case "down", "j":
			if m.focus == focusSidebar {
				if m.cursorFile < len(m.files)-1 {
					m.cursorFile++
					m.viewport.SetContent(m.renderDiff())
					m.viewport.GotoTop()
				}
			} else {
				m.viewport.LineDown(1)
			}

		case "y":
			if m.focus == focusSidebar {
				f := &m.files[m.cursorFile]
				if err := AcceptFile(f); err == nil {
					f.ReviewStatus = StatusAccepted
					m.status = "Accepted file changes."
					m.viewport.SetContent(m.renderDiff())
				}
			} else {
				f := &m.files[m.cursorFile]
				if len(f.Hunks) > 0 {
					h := &f.Hunks[m.cursorHunk]
					if err := AcceptHunk(f, h); err == nil {
						m.viewport.SetContent(m.renderDiff())
						m.status = "Accepted hunk."
					}
				}
			}

		case "x":
			if m.focus == focusSidebar {
				f := &m.files[m.cursorFile]
				if err := RejectFile(f); err == nil {
					f.ReviewStatus = StatusRejected
					m.status = "Rejected file changes."
					m.viewport.SetContent(m.renderDiff())
				}
			} else {
				f := &m.files[m.cursorFile]
				if len(f.Hunks) > 0 {
					h := &f.Hunks[m.cursorHunk]
					if err := RejectHunk(f, h); err == nil {
						m.viewport.SetContent(m.renderDiff())
						m.status = "Rejected hunk."
					}
				}
			}

		case "u":
			if m.focus == focusSidebar {
				f := &m.files[m.cursorFile]
				if err := UnstageFile(f); err == nil {
					m.status = "Moved file back to unreviewed."
					m.viewport.SetContent(m.renderDiff())
				}
			}
		}
	}

	m.viewport, cmd = m.viewport.Update(msg)
	return m, cmd
}

func (m reviewModel) View() string {
	if !m.ready {
		return "\n  Initializing review..."
	}

	if len(m.files) == 0 {
		empty := lipgloss.JoinVertical(lipgloss.Left,
			heroStyle.Render("No code changes yet"),
			"",
			subtleStyle.Render("Press Ctrl+O to open the centered composer and send a new instruction to opencode."),
			subtleStyle.Render("When a run finishes, its diff will appear here automatically."),
		)
		return "\n" + empty
	}

	var sidebar strings.Builder
	sidebar.WriteString(sectionTitleStyle.Render("Files") + "\n\n")

	for i, f := range m.files {
		style := inactiveItemStyle
		if m.cursorFile == i {
			if m.focus == focusSidebar {
				style = activeItemStyle
			} else {
				style = activeItemStyle.Copy().Foreground(textMuted)
			}
		}

		badge := badgeModStyle.Render(" M ")
		if f.Status == "added" {
			badge = badgeAddStyle.Render(" + ")
		} else if f.Status == "deleted" {
			badge = badgeDelStyle.Render(" - ")
		}
		if f.ReviewStatus == StatusAccepted {
			badge = badgeAccStyle.Render(" A ")
		} else if f.ReviewStatus == StatusRejected {
			badge = badgeRejStyle.Render(" R ")
		}

		displayPath := f.NewPath
		if displayPath == "" {
			displayPath = f.OldPath
		}
		if len(displayPath) > 24 {
			displayPath = "..." + displayPath[len(displayPath)-21:]
		}

		sidebar.WriteString(style.Render(badge+displayPath) + "\n")
	}

	sidebarStr := sidebarStyleInactive.Render(sidebar.String())
	if m.focus == focusSidebar {
		sidebarStr = sidebarStyleActive.Render(sidebar.String())
	}

	currFile := m.files[m.cursorFile]
	titlePath := currFile.NewPath
	if titlePath == "" {
		titlePath = currFile.OldPath
	}

	scrollPercent := m.viewport.ScrollPercent()
	scrollText := fmt.Sprintf("  %3.0f%%", scrollPercent*100)
	diffTitle := heroStyle.Render(titlePath) + subtleStyle.Render(scrollText) + "\n" + subtleStyle.Render(m.status) + "\n\n"
	diffView := diffTitle + m.viewport.View()
	mainContent := lipgloss.JoinHorizontal(lipgloss.Top, sidebarStr, diffView)

	modeStr := " FILES "
	modeStyle := statusIdleStyle
	if m.focus == focusViewport {
		modeStr = " HUNKS "
		modeStyle = statusBusyStyle
	}

	footerText := "Ctrl+O: prompt | Enter: open diff | y: accept | x: reject | u: undo"
	if m.focus == focusViewport {
		footerText = "Ctrl+O: prompt | Tab: next hunk | y: accept hunk | x: reject hunk | Esc: files"
	}
	footer := lipgloss.JoinHorizontal(lipgloss.Top, modeStyle.Render(modeStr), subtleStyle.Render(" "+footerText))

	return fmt.Sprintf("\n%s\n\n%s", mainContent, footer)
}
