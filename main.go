package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"os/exec"
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
	Header   string
	OldStart int
	OldCount int
	NewStart int
	NewCount int
	Lines    []DiffLine
}

type FileDiff struct {
	OldPath  string
	NewPath  string
	Status   string
	IsBinary bool
	Hunks    []Hunk
}

// CollectGitDiff runs 'git diff' in the specified repository path and returns the raw output.
func CollectGitDiff(ctx context.Context, repoPath string) (string, error) {
	args := []string{"diff", "--no-color"}
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = repoPath

	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("failed to run git diff: %w", err)
	}

	return string(out), nil
}

func main() {
	cwd, err := os.Getwd()
	if err != nil {
		log.Fatalf("Failed to get current working directory: %v", err)
	}

	diff, err := CollectGitDiff(context.Background(), cwd)
	if err != nil {
		log.Fatalf("Error collecting git diff: %v", err)
	}

	if diff == "" {
		fmt.Println("No uncommitted changes found.")
		return
	}

	parsedFiles, err := ParseGitDiff(diff)
	if err != nil {
		log.Fatalf("Error parsing git diff: %v", err)
	}

	fmt.Printf("Found %d changed files.\n", len(parsedFiles))
	for _, f := range parsedFiles {
		fmt.Printf("- %s (Hunks: %d)\n", f.NewPath, len(f.Hunks))
		for _, h := range f.Hunks {
			fmt.Printf("  - %s (%d lines)\n", h.Header, len(h.Lines))
		}
	}
}
