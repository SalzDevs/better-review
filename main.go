package main

import (
	"flag"
	"fmt"
	"log"
	"os"
	"strings"

	tea "github.com/charmbracelet/bubbletea"
)

type FileStatus string
type LineKind string

type DiffLine struct {
	Kind    string
	Content string
	OldLine int
	NewLine int
}

type Hunk struct {
	Header       string
	OldStart     int
	OldCount     int
	NewStart     int
	NewCount     int
	Lines        []DiffLine
	ReviewStatus ReviewStatus
}

type FileDiff struct {
	OldPath      string
	NewPath      string
	Status       string
	IsBinary     bool
	Hunks        []Hunk
	ReviewStatus ReviewStatus
}

func main() {
	opencodeFlag := flag.String("opencode-bin", "", "path to the opencode binary")
	modelFlag := flag.String("model", "", "default opencode model in provider/model format")
	flag.Parse()

	if err := initLogger(); err != nil {
		fmt.Fprintf(os.Stderr, "warning: failed to initialize debug log: %v\n", err)
	} else {
		defer closeLogger()
	}

	cwd, err := os.Getwd()
	if err != nil {
		log.Fatalf("Error: %v", err)
	}

	runner := NewOpencodeRunner(cwd, *opencodeFlag, *modelFlag)
	p := tea.NewProgram(newAppModel(cwd, runner, strings.TrimSpace(*modelFlag)), tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		log.Fatalf("Error: %v", err)
	}
}
