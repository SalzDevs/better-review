import { Effect } from "effect";

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  content: string[];
}

export interface FileChange {
  path: string;
  oldPath?: string;
  status: "added" | "modified" | "deleted" | "renamed";
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
  binary?: boolean;
}

const DIFF_HEADER_REGEX = /^diff --git a\/(.+?) b\/(.+?)$/m;
const NEW_FILE_REGEX = /^new file mode/m;
const DELETED_FILE_REGEX = /^deleted file mode/m;
const RENAME_FROM_REGEX = /^rename from (.+)$/m;
const RENAME_TO_REGEX = /^rename to (.+)$/m;
const BINARY_REGEX = /^Binary files/m;
const HUNK_REGEX = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

const parseHunk = (lines: string[], lineOffset: number): DiffHunk | Error => {
  if (lines.length === 0) {
    return new Error(`Empty hunk at line ${lineOffset}`);
  }

  const hunkHeader = lines[0];
  const match = hunkHeader.match(HUNK_REGEX);
  if (!match) {
    return new Error(`Invalid hunk header at line ${lineOffset}: "${hunkHeader}"`);
  }

  const oldStart = parseInt(match[1], 10);
  const oldLines = match[2] ? parseInt(match[2], 10) : 1;
  const newStart = parseInt(match[3], 10);
  const newLines = match[4] ? parseInt(match[4], 10) : 1;

  return {
    oldStart,
    oldLines,
    newStart,
    newLines,
    content: lines.slice(1),
  };
};

export const parseDiff = (diff: string): Effect.Effect<[FileChange[], Error[]], Error, never> =>
  Effect.try({
    try: () => {
      const changes: FileChange[] = [];
      const errors: Error[] = [];

      if (!diff.trim()) {
        return [changes, errors];
      }

      const fileBlocks = diff.split(/(?=^diff --git )/m).filter(Boolean);

      for (const block of fileBlocks) {
        const lines = block.split("\n");
        const headerLine = lines[0] || "";

        let path = "";
        let status: FileChange["status"] = "modified";
        let oldPath: string | undefined;
        let binary = false;
        let hunks: DiffHunk[] = [];
        let additions = 0;
        let deletions = 0;

        const pathMatch = headerLine.match(DIFF_HEADER_REGEX);
        if (!pathMatch) {
          errors.push(new Error(`Missing path header at line 1 in block starting with: ${headerLine.slice(0, 50)}`));
          continue;
        }
        path = pathMatch[2];

        for (let i = 0; i < lines.length; i++) {
          const line = lines[i];
          if (NEW_FILE_REGEX.test(line)) {
            status = "added";
          } else if (DELETED_FILE_REGEX.test(line)) {
            status = "deleted";
          } else if (RENAME_FROM_REGEX.test(line)) {
            status = "renamed";
            oldPath = line.match(RENAME_FROM_REGEX)?.[1];
          } else if (RENAME_TO_REGEX.test(line)) {
            path = line.match(RENAME_TO_REGEX)?.[1] || path;
          } else if (BINARY_REGEX.test(line)) {
            binary = true;
          }
        }

        if (!binary) {
          let currentHunkLines: string[] = [];
          let inHunk = false;

          for (let i = 0; i < lines.length; i++) {
            const line = lines[i];
            if (HUNK_REGEX.test(line)) {
              if (currentHunkLines.length > 0) {
                const hunk = parseHunk(currentHunkLines, i);
                if (hunk instanceof Error) {
                  errors.push(hunk);
                } else {
                  hunks.push(hunk);
                }
              }
              currentHunkLines = [line];
              inHunk = true;
            } else if (inHunk) {
              currentHunkLines.push(line);
            }
          }

          if (currentHunkLines.length > 0) {
            const hunk = parseHunk(currentHunkLines, lines.length);
            if (hunk instanceof Error) {
              errors.push(hunk);
            } else {
              hunks.push(hunk);
            }
          }

          for (const hunk of hunks) {
            for (const line of hunk.content) {
              if (line.startsWith("+") && !line.startsWith("+++")) {
                additions++;
              } else if (line.startsWith("-") && !line.startsWith("---")) {
                deletions++;
              }
            }
          }
        }

        changes.push({
          path,
          oldPath,
          status,
          hunks,
          additions,
          deletions,
          binary,
        });
      }

      return [changes, errors];
    },
    catch: (error: unknown) => new Error(`parseDiff failed: ${error}`),
  });
