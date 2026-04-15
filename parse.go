package main

import (
	"regexp"
	"strconv"
	"strings"
)

var hunkHeaderRe = regexp.MustCompile(`^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)`)

func ParseGitDiff(diff string) ([]FileDiff, error) {
	var files []FileDiff
	var currentFile *FileDiff
	var currentHunk *Hunk
	var currentOldLine, currentNewLine int

	lines := strings.Split(diff, "\n")

	for _, line := range lines {
		if strings.HasPrefix(line, "diff --git ") {
			if currentHunk != nil && currentFile != nil {
				currentFile.Hunks = append(currentFile.Hunks, *currentHunk)
				currentHunk = nil
			}
			if currentFile != nil {
				files = append(files, *currentFile)
			}
			currentFile = &FileDiff{}
			parts := strings.SplitN(line, " ", 4)
			if len(parts) == 4 {
				currentFile.OldPath = strings.TrimPrefix(parts[2], "a/")
				currentFile.NewPath = strings.TrimPrefix(parts[3], "b/")

				if parts[2] == "/dev/null" {
					currentFile.Status = "added"
					currentFile.OldPath = ""
				} else if parts[3] == "/dev/null" {
					currentFile.Status = "deleted"
					currentFile.NewPath = ""
				} else {
					currentFile.Status = "modified"
				}
			}
			continue
		}

		if currentFile == nil {
			continue
		}

		if strings.HasPrefix(line, "--- ") || strings.HasPrefix(line, "+++ ") {
			continue
		}

		if strings.HasPrefix(line, "@@ ") {
			if currentHunk != nil {
				currentFile.Hunks = append(currentFile.Hunks, *currentHunk)
			}
			matches := hunkHeaderRe.FindStringSubmatch(line)
			if len(matches) > 0 {
				oldStart, _ := strconv.Atoi(matches[1])
				oldCount := 1
				if matches[2] != "" {
					oldCount, _ = strconv.Atoi(matches[2])
				}
				newStart, _ := strconv.Atoi(matches[3])
				newCount := 1
				if matches[4] != "" {
					newCount, _ = strconv.Atoi(matches[4])
				}

				currentHunk = &Hunk{
					Header:   line,
					OldStart: oldStart,
					OldCount: oldCount,
					NewStart: newStart,
					NewCount: newCount,
					Lines:    []DiffLine{},
				}
				currentOldLine = oldStart
				currentNewLine = newStart
			}
			continue
		}

		if currentHunk != nil {
			if len(line) == 0 {
				continue
			}
			prefix := line[0:1]
			content := line
			if len(line) > 0 {
				content = line[1:]
			}

			kind := ""
			oldL := 0
			newL := 0

			switch prefix {
			case "+":
				kind = "add"
				newL = currentNewLine
				currentNewLine++
			case "-":
				kind = "remove"
				oldL = currentOldLine
				currentOldLine++
			case " ":
				kind = "context"
				oldL = currentOldLine
				newL = currentNewLine
				currentOldLine++
				currentNewLine++
			}

			if kind != "" {
				currentHunk.Lines = append(currentHunk.Lines, DiffLine{
					Kind:    kind,
					Content: content,
					OldLine: oldL,
					NewLine: newL,
				})
			}
		}
	}

	if currentHunk != nil && currentFile != nil {
		currentFile.Hunks = append(currentFile.Hunks, *currentHunk)
	}
	if currentFile != nil {
		files = append(files, *currentFile)
	}

	return files, nil
}
