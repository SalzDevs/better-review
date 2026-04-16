package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/muesli/termenv"
)

var debugLog *os.File

func init() {
	lipgloss.SetColorProfile(termenv.ANSI256)
}

var (
	baseBackground  = lipgloss.Color("#101418")
	panelBackground = lipgloss.Color("#171f24")
	panelMuted      = lipgloss.Color("#22303a")
	textPrimary     = lipgloss.Color("#edf2f7")
	textMuted       = lipgloss.Color("#93a6b5")
	textSubtle      = lipgloss.Color("#607282")
	accentColor     = lipgloss.Color("#f4b942")
	accentDeep      = lipgloss.Color("#c98d1d")
	dangerColor     = lipgloss.Color("#ef6f6c")
	successColor    = lipgloss.Color("#59c9a5")

	shellStyle  = lipgloss.NewStyle().Background(baseBackground).Foreground(textPrimary)
	heroStyle   = lipgloss.NewStyle().Foreground(textPrimary).Bold(true)
	subtleStyle = lipgloss.NewStyle().
			Foreground(textMuted)

	statusIdleStyle  = lipgloss.NewStyle().Foreground(textPrimary).Background(panelMuted).Bold(true).Padding(0, 1)
	statusBusyStyle  = lipgloss.NewStyle().Foreground(baseBackground).Background(accentColor).Bold(true).Padding(0, 1)
	statusErrorStyle = lipgloss.NewStyle().Foreground(textPrimary).Background(dangerColor).Bold(true).Padding(0, 1)

	sectionTitleStyle = lipgloss.NewStyle().Foreground(accentColor).Bold(true)
	hintStyle         = lipgloss.NewStyle().Foreground(textSubtle)
	shortcutStyle     = lipgloss.NewStyle().Foreground(accentColor).Bold(true)

	inputStyle       = lipgloss.NewStyle().Foreground(textPrimary)
	inputPromptStyle = lipgloss.NewStyle().Foreground(accentColor).Bold(true)

	modalBackdropStyle = lipgloss.NewStyle().Background(lipgloss.Color("#0b0f13")).Foreground(textPrimary)
	modalCardStyle     = lipgloss.NewStyle().
				Background(panelBackground).
				Border(lipgloss.RoundedBorder()).
				BorderForeground(accentDeep).
				Padding(1, 2)

	inputBorderStyle = lipgloss.NewStyle().
				Border(lipgloss.RoundedBorder()).
				BorderForeground(accentDeep).
				Background(panelBackground).
				Padding(0, 1)

	modelRowStyle       = lipgloss.NewStyle().Padding(0, 1)
	modelRowActiveStyle = lipgloss.NewStyle().
				Padding(0, 1).
				Background(lipgloss.Color("#202a31")).
				Foreground(textPrimary).
				Bold(true)

	selectedTagStyle = lipgloss.NewStyle().Foreground(baseBackground).Background(successColor).Bold(true).Padding(0, 1)
	providerStyle    = lipgloss.NewStyle().Foreground(textSubtle)
	errorTextStyle   = lipgloss.NewStyle().Foreground(dangerColor)
	footerBarStyle   = lipgloss.NewStyle().Foreground(textMuted)
)

type runStatus int

const (
	statusIdle runStatus = iota
	statusRunning
	statusFailed
)

type overlayKind int

const (
	overlayNone overlayKind = iota
	overlayPrompt
	overlayModelPicker
)

type promptRun struct {
	Prompt         string
	StartedAt      time.Time
	FinishedAt     time.Time
	ChangedFiles   int
	HasDiff        bool
	FailureMessage string
	Command        string
	Model          string
	Variant        string
}

type opencodeRunResult struct {
	Run    promptRun
	Files  []FileDiff
	Err    error
	Stdout string
	Stderr string
}

type runFinishedMsg struct {
	result opencodeRunResult
}

type modelsLoadedMsg struct {
	models []ModelOption
	err    error
}

type ModelOption struct {
	ID       string
	Provider string
	Name     string
	Variants []string
}

type appModel struct {
	repoPath          string
	runner            *OpencodeRunner
	review            reviewModel
	runStatus         runStatus
	ready             bool
	width             int
	height            int
	statusMessage     string
	lastRun           *promptRun
	runCounter        int
	overlay           overlayKind
	promptInput       textinput.Model
	modelSearchInput  textinput.Model
	selectedModel     string
	selectedVariant   string
	availableModels   []ModelOption
	filteredModels    []ModelOption
	modelCursor       int
	modelsLoaded      bool
	modelLoadError    string
	showVariantPicker bool
	variantCursor     int
}

func newAppModel(repoPath string, runner *OpencodeRunner, initialModel string) appModel {
	promptInput := textinput.New()
	promptInput.Prompt = ""
	promptInput.Placeholder = "Describe the change you want opencode to make"
	promptInput.CharLimit = 0
	promptInput.TextStyle = inputStyle
	promptInput.PromptStyle = inputPromptStyle

	modelSearch := textinput.New()
	modelSearch.Prompt = ""
	modelSearch.Placeholder = "Search models"
	modelSearch.CharLimit = 0
	modelSearch.TextStyle = inputStyle
	modelSearch.PromptStyle = inputPromptStyle

	return appModel{
		repoPath:         repoPath,
		runner:           runner,
		review:           newReviewModel(nil),
		runStatus:        statusIdle,
		statusMessage:    "Press Ctrl+O to open the prompt composer.",
		promptInput:      promptInput,
		modelSearchInput: modelSearch,
		selectedModel:    initialModel,
	}
}

func (m appModel) Init() tea.Cmd {
	return tea.Batch(textinput.Blink, m.loadModelsCmd())
}

func (m appModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		m.ready = true
		m.resize()

	case modelsLoadedMsg:
		if msg.err != nil {
			m.modelLoadError = msg.err.Error()
			m.statusMessage = "Could not load opencode models."
			return m, nil
		}
		m.availableModels = msg.models
		m.modelsLoaded = true
		m.filterModels()
		m.ensureSelectedModel()

	case runFinishedMsg:
		m.runStatus = statusIdle
		if msg.result.Err != nil {
			m.runStatus = statusFailed
			failedRun := msg.result.Run
			failedRun.FailureMessage = msg.result.Err.Error()
			m.lastRun = &failedRun
			m.statusMessage = failedRun.FailureMessage
			m.overlay = overlayNone
			return m, nil
		}

		finishedRun := msg.result.Run
		m.lastRun = &finishedRun
		m.overlay = overlayNone
		if finishedRun.HasDiff {
			m.review = newReviewModel(msg.result.Files)
			m.review.resize(m.width, m.height)
			m.statusMessage = fmt.Sprintf("Run %d finished. Review %d changed file(s).", m.runCounter, finishedRun.ChangedFiles)
		} else {
			m.review = newReviewModel(nil)
			m.review.resize(m.width, m.height)
			m.statusMessage = "Run finished with no code changes."
		}

	case tea.KeyMsg:
		switch m.overlay {
		case overlayPrompt:
			return m.updatePromptOverlay(msg)
		case overlayModelPicker:
			return m.updateModelOverlay(msg)
		}

		switch msg.String() {
		case "ctrl+c":
			return m, tea.Quit
		case "ctrl+o":
			m.openPromptOverlay()
			return m, nil
		}
	}

	updatedReview, cmd := m.review.Update(msg)
	m.review = updatedReview
	if cmd != nil {
		cmds = append(cmds, cmd)
	}

	return m, tea.Batch(cmds...)
}

func (m appModel) updatePromptOverlay(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "ctrl+c":
		return m, tea.Quit
	case "esc":
		m.closeOverlay()
		return m, nil
	case "ctrl+o":
		m.closeOverlay()
		return m, nil
	case "tab":
		m.overlay = overlayModelPicker
		m.modelSearchInput.Focus()
		m.promptInput.Blur()
		m.filterModels()
		return m, nil
	case "shift+tab":
		m.cycleVariant(-1)
		return m, nil
	case "ctrl+t":
		m.cycleVariant(1)
		return m, nil
	case "enter":
		if m.runStatus == statusRunning {
			return m, nil
		}
		prompt := strings.TrimSpace(m.promptInput.Value())
		if prompt == "" {
			m.statusMessage = "Write a prompt first."
			return m, nil
		}
		m.runCounter++
		m.runStatus = statusRunning
		m.statusMessage = fmt.Sprintf("Running opencode with %s...", m.currentModelLabel())
		m.promptInput.SetValue("")
		return m, m.runPromptCmd(prompt)
	}

	var cmd tea.Cmd
	m.promptInput, cmd = m.promptInput.Update(msg)
	return m, cmd
}

func (m appModel) updateModelOverlay(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch msg.String() {
	case "ctrl+c":
		return m, tea.Quit
	case "esc":
		m.overlay = overlayPrompt
		m.promptInput.Focus()
		m.modelSearchInput.Blur()
		return m, nil
	case "up", "k":
		if m.modelCursor > 0 {
			m.modelCursor--
		}
		return m, nil
	case "down", "j":
		if m.modelCursor < len(m.filteredModels)-1 {
			m.modelCursor++
		}
		return m, nil
	case "enter":
		if len(m.filteredModels) == 0 {
			return m, nil
		}
		selected := m.filteredModels[m.modelCursor]
		m.selectedModel = selected.ID
		m.selectedVariant = firstVariant(selected.Variants)
		m.runner.SetModelSelection(m.selectedModel, m.selectedVariant)
		m.overlay = overlayPrompt
		m.promptInput.Focus()
		m.modelSearchInput.Blur()
		m.statusMessage = fmt.Sprintf("Selected model %s.", m.currentModelLabel())
		return m, nil
	}

	var cmd tea.Cmd
	m.modelSearchInput, cmd = m.modelSearchInput.Update(msg)
	m.filterModels()
	return m, cmd
}

func (m appModel) View() string {
	if !m.ready {
		return shellStyle.Render("\n  Loading better-review...")
	}

	base := m.renderBaseView()
	if m.overlay == overlayNone {
		return shellStyle.Render(base)
	}

	return shellStyle.Render(m.renderOverlay(base))
}

func (m *appModel) resize() {
	if m.width <= 0 || m.height <= 0 {
		return
	}

	m.review.resize(m.width, m.height)
	promptWidth := min(max(m.width/2, 48), 96)
	m.promptInput.Width = promptWidth - 8
	m.modelSearchInput.Width = promptWidth - 8
}

func (m *appModel) renderBaseView() string {
	header := m.renderHeader()
	reviewView := m.review.View()
	footer := m.renderFooter()
	return lipgloss.JoinVertical(lipgloss.Left, header, "", reviewView, "", footer)
}

func (m *appModel) renderHeader() string {
	status := statusIdleStyle.Render("READY")
	switch m.runStatus {
	case statusRunning:
		status = statusBusyStyle.Render("RUNNING")
	case statusFailed:
		status = statusErrorStyle.Render("FAILED")
	}

	projectName := filepath.Base(m.repoPath)
	title := heroStyle.Render("better-review") + "  " + subtleStyle.Render(projectName)
	modelLine := subtleStyle.Render("Model: ") + heroStyle.Render(m.currentModelLabel())
	meta := subtleStyle.Render(m.statusMessage)

	card := modalCardStyle.Copy().
		Width(max(32, m.width-4)).
		BorderForeground(panelMuted)

	return card.Render(lipgloss.JoinVertical(lipgloss.Left,
		lipgloss.JoinHorizontal(lipgloss.Center, title, "   ", status),
		"",
		modelLine,
		meta,
	))
}

func (m *appModel) renderFooter() string {
	left := footerBarStyle.Render("Ctrl+O composer  |  Enter diff  |  Esc files  |  y accept  |  x reject")
	right := footerBarStyle.Render("Ctrl+C quit")
	gap := max(1, m.width-lipgloss.Width(left)-lipgloss.Width(right)-4)
	return lipgloss.JoinHorizontal(lipgloss.Left, left, strings.Repeat(" ", gap), right)
}

func (m *appModel) renderOverlay(base string) string {
	scrim := lipgloss.NewStyle().Width(m.width).Height(m.height).Render(base)
	modal := ""
	if m.overlay == overlayPrompt {
		modal = m.renderPromptModal()
	} else if m.overlay == overlayModelPicker {
		modal = m.renderModelPickerModal()
	}
	return placeOverlay(scrim, modal, m.width, m.height)
}

func (m *appModel) renderPromptModal() string {
	currentModel := lipgloss.JoinHorizontal(lipgloss.Left,
		sectionTitleStyle.Render("Model"),
		"  ",
		heroStyle.Render(m.currentModelLabel()),
	)
	if m.selectedVariant != "" {
		currentModel += "  " + subtleStyle.Render("variant: "+m.selectedVariant)
	}

	hints := lipgloss.JoinHorizontal(lipgloss.Left,
		hintStyle.Render("Enter run  |  "),
		hintStyle.Render("Tab models  |  "),
		hintStyle.Render("Ctrl+T cycle variant  |  "),
		hintStyle.Render("Esc close"),
	)

	body := lipgloss.JoinVertical(lipgloss.Left,
		sectionTitleStyle.Render("Prompt opencode"),
		"",
		currentModel,
		"",
		hints,
		"",
		inputBorderStyle.Width(max(32, min(m.width/2, 96))).Render(inputPromptStyle.Render("> ")+m.promptInput.View()),
	)

	if m.modelLoadError != "" {
		body += "\n\n" + errorTextStyle.Render(m.modelLoadError)
	}

	width := min(max(m.width/2, 48), 96)
	return modalCardStyle.Width(width).Render(body)
}

func (m *appModel) renderModelPickerModal() string {
	rows := []string{sectionTitleStyle.Render("Choose model"), "", subtleStyle.Render("Search and press Enter to select. Esc returns to the composer."), ""}
	rows = append(rows, inputBorderStyle.Width(max(32, min(m.width/2, 96))).Render(inputPromptStyle.Render("/ ")+m.modelSearchInput.View()))
	rows = append(rows, "")

	if len(m.filteredModels) == 0 {
		rows = append(rows, subtleStyle.Render("No models match your search."))
	} else {
		for i, model := range m.filteredModels {
			label := formatModelOption(model)
			if model.ID == m.selectedModel {
				label += "  " + selectedTagStyle.Render("selected")
			}
			rowStyle := modelRowStyle
			if i == m.modelCursor {
				rowStyle = modelRowActiveStyle
			}
			rows = append(rows, rowStyle.Render(label))
		}
	}

	width := min(max(m.width/2, 56), 100)
	return modalCardStyle.Width(width).Render(lipgloss.JoinVertical(lipgloss.Left, rows...))
}

func (m *appModel) openPromptOverlay() {
	m.overlay = overlayPrompt
	m.promptInput.Focus()
	m.modelSearchInput.Blur()
	m.statusMessage = "Compose a new prompt."
}

func (m *appModel) closeOverlay() {
	m.overlay = overlayNone
	m.promptInput.Blur()
	m.modelSearchInput.Blur()
	m.statusMessage = "Review remains active. Press Ctrl+O for a new prompt."
}

func (m *appModel) runPromptCmd(prompt string) tea.Cmd {
	selectedModel := m.selectedModel
	selectedVariant := m.selectedVariant
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
		defer cancel()

		result := m.runner.RunPrompt(ctx, prompt, m.runCounter, selectedModel, selectedVariant)
		return runFinishedMsg{result: result}
	}
}

func (m *appModel) loadModelsCmd() tea.Cmd {
	return func() tea.Msg {
		models, err := m.runner.LoadModels(context.Background())
		return modelsLoadedMsg{models: models, err: err}
	}
}

func (m *appModel) filterModels() {
	query := strings.ToLower(strings.TrimSpace(m.modelSearchInput.Value()))
	if len(m.availableModels) == 0 {
		m.filteredModels = nil
		m.modelCursor = 0
		return
	}

	filtered := make([]ModelOption, 0, len(m.availableModels))
	for _, model := range m.availableModels {
		if query == "" || strings.Contains(strings.ToLower(model.ID), query) || strings.Contains(strings.ToLower(model.Name), query) {
			filtered = append(filtered, model)
		}
	}
	m.filteredModels = filtered
	if len(filtered) == 0 {
		m.modelCursor = 0
		return
	}
	if m.modelCursor >= len(filtered) {
		m.modelCursor = len(filtered) - 1
	}
	if m.modelCursor < 0 {
		m.modelCursor = 0
	}
}

func (m *appModel) ensureSelectedModel() {
	if m.selectedModel != "" {
		for _, model := range m.availableModels {
			if model.ID == m.selectedModel {
				if m.selectedVariant == "" {
					m.selectedVariant = firstVariant(model.Variants)
				}
				m.runner.SetModelSelection(m.selectedModel, m.selectedVariant)
				return
			}
		}
	}
	if len(m.availableModels) == 0 {
		return
	}
	m.selectedModel = m.availableModels[0].ID
	m.selectedVariant = firstVariant(m.availableModels[0].Variants)
	m.runner.SetModelSelection(m.selectedModel, m.selectedVariant)
}

func (m *appModel) cycleVariant(direction int) {
	variants := m.selectedModelVariants()
	if len(variants) == 0 {
		m.selectedVariant = ""
		return
	}
	index := 0
	for i, variant := range variants {
		if variant == m.selectedVariant {
			index = i
			break
		}
	}
	index = (index + direction + len(variants)) % len(variants)
	m.selectedVariant = variants[index]
	m.runner.SetModelSelection(m.selectedModel, m.selectedVariant)
}

func (m *appModel) selectedModelVariants() []string {
	for _, model := range m.availableModels {
		if model.ID == m.selectedModel {
			return model.Variants
		}
	}
	return nil
}

func (m *appModel) currentModelLabel() string {
	if m.selectedVariant == "" {
		return m.selectedModel
	}
	return m.selectedModel + " [" + m.selectedVariant + "]"
}

type OpencodeRunner struct {
	repoPath       string
	binary         string
	defaultModel   string
	defaultVariant string
}

func NewOpencodeRunner(repoPath, requestedBinary, requestedModel string) *OpencodeRunner {
	binary := strings.TrimSpace(requestedBinary)
	if binary == "" {
		binary = defaultOpencodeBinary(repoPath)
	}
	return &OpencodeRunner{repoPath: repoPath, binary: binary, defaultModel: strings.TrimSpace(requestedModel)}
}

func defaultOpencodeBinary(repoPath string) string {
	return "opencode"
}

func (r *OpencodeRunner) SetModelSelection(modelID, variant string) {
	r.defaultModel = strings.TrimSpace(modelID)
	r.defaultVariant = strings.TrimSpace(variant)
}

func (r *OpencodeRunner) RunPrompt(ctx context.Context, prompt string, runNumber int, modelID, variant string) opencodeRunResult {
	startedAt := time.Now()
	result := opencodeRunResult{
		Run: promptRun{
			Prompt:    prompt,
			StartedAt: startedAt,
			Command:   r.commandLabel(),
			Model:     modelID,
			Variant:   variant,
		},
	}

	beforeDiff, err := CollectGitDiff(ctx, r.repoPath)
	if err != nil {
		result.Run.FinishedAt = time.Now()
		result.Err = err
		return result
	}

	stdout, stderr, err := r.execute(ctx, prompt, modelID, variant)
	result.Stdout = stdout
	result.Stderr = stderr
	if stdout != "" || stderr != "" {
		log.Printf("run %d completed command %q (stdout=%d bytes, stderr=%d bytes)", runNumber, r.commandLabel(), len(stdout), len(stderr))
	}
	if err != nil {
		result.Run.FinishedAt = time.Now()
		result.Err = fmt.Errorf("opencode run failed: %w", err)
		return result
	}

	afterDiff, err := CollectGitDiff(ctx, r.repoPath)
	if err != nil {
		result.Run.FinishedAt = time.Now()
		result.Err = err
		return result
	}

	files, err := ParseGitDiff(afterDiff)
	if err != nil {
		result.Run.FinishedAt = time.Now()
		result.Err = fmt.Errorf("parse git diff: %w", err)
		return result
	}

	result.Run.FinishedAt = time.Now()
	result.Run.HasDiff = strings.TrimSpace(afterDiff) != "" && len(files) > 0
	result.Run.ChangedFiles = len(files)
	result.Files = files
	if beforeDiff == afterDiff && result.Run.HasDiff {
		log.Printf("run %d finished without changing the existing diff", runNumber)
	}
	return result
}

func (r *OpencodeRunner) execute(ctx context.Context, prompt, modelID, variant string) (string, string, error) {
	args := []string{"run", "--dir", r.repoPath, "--format", "json"}
	if modelID != "" {
		args = append(args, "--model", modelID)
	}
	if variant != "" {
		args = append(args, "--variant", variant)
	}
	args = append(args, prompt)

	cmd := exec.CommandContext(ctx, r.binary, args...)
	cmd.Dir = r.repoPath

	var stdoutBuf strings.Builder
	var stderrBuf strings.Builder
	cmd.Stdout = &stdoutBuf
	cmd.Stderr = &stderrBuf
	err := cmd.Run()
	if err == nil {
		return stdoutBuf.String(), stderrBuf.String(), nil
	}

	var exitErr *exec.ExitError
	if errors.As(err, &exitErr) {
		return stdoutBuf.String(), stderrBuf.String(), err
	}
	if stdoutBuf.Len() == 0 && stderrBuf.Len() == 0 {
		stderrBuf.WriteString(err.Error())
	}
	return stdoutBuf.String(), stderrBuf.String(), err
}

func (r *OpencodeRunner) LoadModels(ctx context.Context) ([]ModelOption, error) {
	cmd := exec.CommandContext(ctx, r.binary, "models", "--verbose")
	cmd.Dir = r.repoPath
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("load models: %w", err)
	}
	return parseModelOptions(string(out)), nil
}

func (r *OpencodeRunner) commandLabel() string {
	base := filepath.Base(r.binary)
	if base == "." || base == string(filepath.Separator) || base == "" {
		return r.binary
	}
	return base + " run"
}

func parseModelOptions(raw string) []ModelOption {
	chunks := strings.Split(strings.TrimSpace(raw), "\n\n")
	options := make([]ModelOption, 0, len(chunks))
	for _, chunk := range chunks {
		chunk = strings.TrimSpace(chunk)
		if chunk == "" {
			continue
		}
		lines := strings.Split(chunk, "\n")
		id := strings.TrimSpace(lines[0])
		if id == "" || !strings.Contains(id, "/") {
			continue
		}
		provider, name := splitModelID(id)
		option := ModelOption{ID: id, Provider: provider, Name: name}
		option.Variants = parseVariantsFromModelChunk(lines[1:])
		options = append(options, option)
	}
	sort.Slice(options, func(i, j int) bool {
		return options[i].ID < options[j].ID
	})
	return options
}

func parseVariantsFromModelChunk(lines []string) []string {
	if len(lines) == 0 {
		return nil
	}

	body := strings.TrimSpace(strings.Join(lines, "\n"))
	if body == "" {
		return nil
	}

	decoder := json.NewDecoder(bytes.NewBufferString(body))
	decoder.UseNumber()
	var payload struct {
		Variants map[string]json.RawMessage `json:"variants"`
	}
	if err := decoder.Decode(&payload); err != nil {
		return nil
	}

	variants := make([]string, 0, len(payload.Variants))
	for name := range payload.Variants {
		variants = append(variants, name)
	}
	return uniqueSorted(variants)
}

func splitModelID(id string) (string, string) {
	parts := strings.SplitN(id, "/", 2)
	if len(parts) != 2 {
		return "", id
	}
	return parts[0], parts[1]
}

func formatModelOption(option ModelOption) string {
	label := heroStyle.Render(option.ID)
	if len(option.Variants) > 0 {
		label += "  " + providerStyle.Render("variants: "+strings.Join(option.Variants, ", "))
	}
	return label
}

func firstVariant(variants []string) string {
	if len(variants) == 0 {
		return ""
	}
	return variants[0]
}

func uniqueSorted(values []string) []string {
	if len(values) == 0 {
		return nil
	}
	seen := map[string]struct{}{}
	result := make([]string, 0, len(values))
	for _, value := range values {
		if _, ok := seen[value]; ok {
			continue
		}
		seen[value] = struct{}{}
		result = append(result, value)
	}
	sort.Strings(result)
	return result
}

func placeOverlay(base, modal string, width, height int) string {
	baseLines := strings.Split(base, "\n")
	modalLines := strings.Split(modal, "\n")
	startY := max(1, height/4)
	startX := max(2, (width-lipgloss.Width(modal))/2)

	for i, line := range modalLines {
		target := startY + i
		if target < 0 || target >= len(baseLines) {
			continue
		}
		prefix := padRight("", startX)
		baseLines[target] = overlayLine(baseLines[target], prefix+line, startX)
	}

	return strings.Join(baseLines, "\n")
}

func overlayLine(base, overlay string, startX int) string {
	baseRunes := []rune(base)
	overlayRunes := []rune(overlay)
	if len(baseRunes) < len(overlayRunes) {
		baseRunes = append(baseRunes, []rune(strings.Repeat(" ", len(overlayRunes)-len(baseRunes)))...)
	}
	for i, r := range overlayRunes {
		if startX+i >= len(baseRunes) {
			break
		}
		baseRunes[startX+i] = r
	}
	return string(baseRunes)
}

func padRight(value string, width int) string {
	current := lipgloss.Width(value)
	if current >= width {
		return value
	}
	return value + strings.Repeat(" ", width-current)
}

func CollectGitDiff(ctx context.Context, repoPath string) (string, error) {
	trackedDiff, err := runCommandOutput(ctx, repoPath, "git", "diff", "--no-color", "--no-ext-diff")
	if err != nil {
		return "", fmt.Errorf("failed to run git diff: %w", err)
	}

	untrackedDiff, err := collectUntrackedDiff(ctx, repoPath)
	if err != nil {
		return "", err
	}

	if trackedDiff == "" {
		return untrackedDiff, nil
	}
	if untrackedDiff == "" {
		return trackedDiff, nil
	}
	return trackedDiff + "\n" + untrackedDiff, nil
}

func collectUntrackedDiff(ctx context.Context, repoPath string) (string, error) {
	paths, err := listUntrackedFiles(ctx, repoPath)
	if err != nil {
		return "", err
	}

	var diffs []string
	for _, path := range paths {
		diff, err := diffForUntrackedFile(ctx, repoPath, path)
		if err != nil {
			return "", err
		}
		if strings.TrimSpace(diff) != "" {
			diffs = append(diffs, diff)
		}
	}

	return strings.Join(diffs, "\n"), nil
}

func listUntrackedFiles(ctx context.Context, repoPath string) ([]string, error) {
	out, err := runCommandOutput(ctx, repoPath, "git", "ls-files", "--others", "--exclude-standard", "-z")
	if err != nil {
		return nil, fmt.Errorf("list untracked files: %w", err)
	}

	if out == "" {
		return nil, nil
	}

	parts := strings.Split(out, "\x00")
	paths := make([]string, 0, len(parts))
	for _, part := range parts {
		if part == "" {
			continue
		}
		paths = append(paths, part)
	}
	return paths, nil
}

func diffForUntrackedFile(ctx context.Context, repoPath, path string) (string, error) {
	cmd := exec.CommandContext(ctx, "git", "diff", "--no-index", "--no-color", "--", "/dev/null", path)
	cmd.Dir = repoPath

	out, err := cmd.Output()
	if err == nil {
		return string(out), nil
	}

	var exitErr *exec.ExitError
	if errors.As(err, &exitErr) && exitErr.ExitCode() == 1 {
		return string(out), nil
	}

	return "", fmt.Errorf("diff untracked file %s: %w", path, err)
}

func runCommandOutput(ctx context.Context, dir, name string, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return "", err
	}
	return string(out), nil
}

func initLogger() error {
	file, err := os.OpenFile("debug.log", os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0666)
	if err != nil {
		return err
	}
	debugLog = file
	log.SetOutput(file)
	log.SetFlags(log.Ltime | log.Lmicroseconds | log.Lshortfile)
	log.Println("Logger initialized")
	return nil
}

func closeLogger() {
	if debugLog != nil {
		_ = debugLog.Close()
		debugLog = nil
	}
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
