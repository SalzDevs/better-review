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
	if len(os.Args) > 1 && os.Args[1] == "opencode" {
		if err := runProxy(os.Args[1:]); err != nil {
			log.Fatalf("Proxy error: %v", err)
		}
		return
	}

	if err := runReview(); err != nil {
		log.Fatalf("Error: %v", err)
	}
}
