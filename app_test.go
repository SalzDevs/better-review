package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestCollectGitDiffIncludesUntrackedFiles(t *testing.T) {
	repoPath := initTestRepo(t)

	trackedPath := filepath.Join(repoPath, "tracked.txt")
	writeTestFile(t, trackedPath, "before\n")
	runTestCommand(t, repoPath, "git", "add", "tracked.txt")
	runTestCommand(t, repoPath, "git", "commit", "-m", "initial")

	writeTestFile(t, trackedPath, "after\n")
	writeTestFile(t, filepath.Join(repoPath, "new.txt"), "new file\n")

	diff, err := CollectGitDiff(context.Background(), repoPath)
	if err != nil {
		t.Fatalf("CollectGitDiff returned error: %v", err)
	}

	if !strings.Contains(diff, "tracked.txt") {
		t.Fatalf("expected tracked file diff, got %q", diff)
	}
	if !strings.Contains(diff, "new.txt") {
		t.Fatalf("expected untracked file diff, got %q", diff)
	}

	files, err := ParseGitDiff(diff)
	if err != nil {
		t.Fatalf("ParseGitDiff returned error: %v", err)
	}

	if len(files) != 2 {
		t.Fatalf("expected 2 file diffs, got %d", len(files))
	}

	statuses := map[string]string{}
	for _, file := range files {
		path := file.NewPath
		if path == "" {
			path = file.OldPath
		}
		statuses[path] = file.Status
	}

	if statuses["tracked.txt"] != "modified" {
		t.Fatalf("expected tracked.txt to be modified, got %q", statuses["tracked.txt"])
	}
	if statuses["new.txt"] != "added" {
		t.Fatalf("expected new.txt to be added, got %q", statuses["new.txt"])
	}
}

func TestRejectFileRemovesAddedFile(t *testing.T) {
	repoPath := initTestRepo(t)

	addedPath := filepath.Join(repoPath, "new.txt")
	writeTestFile(t, addedPath, "new file\n")

	oldWD, err := os.Getwd()
	if err != nil {
		t.Fatalf("getwd: %v", err)
	}
	defer func() {
		_ = os.Chdir(oldWD)
	}()
	if err := os.Chdir(repoPath); err != nil {
		t.Fatalf("chdir repo: %v", err)
	}

	file := &FileDiff{NewPath: "new.txt", Status: "added", Hunks: []Hunk{{}, {}}}
	if err := RejectFile(file); err != nil {
		t.Fatalf("RejectFile returned error: %v", err)
	}

	if _, err := os.Stat(addedPath); !os.IsNotExist(err) {
		t.Fatalf("expected added file to be removed, stat err=%v", err)
	}
	if file.ReviewStatus != StatusRejected {
		t.Fatalf("expected file status rejected, got %q", file.ReviewStatus)
	}
	for i, hunk := range file.Hunks {
		if hunk.ReviewStatus != StatusRejected {
			t.Fatalf("expected hunk %d rejected, got %q", i, hunk.ReviewStatus)
		}
	}
}

func TestParseModelOptionsExtractsVariants(t *testing.T) {
	raw := strings.TrimSpace(`openai/gpt-5.4
{
  "id": "gpt-5.4",
  "providerID": "openai",
  "variants": {
    "low": {
      "reasoningEffort": "low"
    },
    "high": {
      "reasoningEffort": "high"
    }
  }
}

github-copilot/gpt-5.1-codex
{
  "id": "gpt-5.1-codex",
  "providerID": "github-copilot",
  "variants": {}
}`)

	models := parseModelOptions(raw)
	if len(models) != 2 {
		t.Fatalf("expected 2 models, got %d", len(models))
	}

	if models[1].ID != "openai/gpt-5.4" {
		t.Fatalf("expected sorted openai model last, got %q", models[1].ID)
	}
	if strings.Join(models[1].Variants, ",") != "high,low" {
		t.Fatalf("expected extracted variants, got %v", models[1].Variants)
	}
}

func initTestRepo(t *testing.T) string {
	t.Helper()
	repoPath := t.TempDir()
	runTestCommand(t, repoPath, "git", "init")
	runTestCommand(t, repoPath, "git", "config", "user.name", "Better Review Test")
	runTestCommand(t, repoPath, "git", "config", "user.email", "better-review@example.com")
	return repoPath
}

func writeTestFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatalf("write %s: %v", path, err)
	}
}

func runTestCommand(t *testing.T, dir, name string, args ...string) {
	t.Helper()
	cmd := exec.Command(name, args...)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%s %v failed: %v\n%s", name, args, err, string(out))
	}
}
