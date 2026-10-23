package tools

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

const (
	editLoopLimit     = 5 // attempts per path before the guard refuses
	summaryWriteLimit = 4 // churn-markdown writes before the guard refuses
)

func (s *fileState) isAuthored(path string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.authored[path]
}

func (s *fileState) markRead(path string, step int) {
	s.mu.Lock()
	s.read[path] = step
	s.mu.Unlock()
}

func (s *fileState) wasRead(path string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	_, ok := s.read[path]
	return ok
}

// readInStep says whether the last read of path happened in this very step (so its result was not yet in the model's hands).
func (s *fileState) readInStep(path string, step int) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	at, ok := s.read[path]
	return ok && step > 0 && at == step
}

// toolOutputMarker is text a model writes when it imagines a tool's result in place of the real content.
var toolOutputMarker = regexp.MustCompile(`(?i)<(read_file_output|tool_output|tool_result|function_results?)\b|"tool":\s*"read_file"`)

func (s *fileState) markAuthored(path string) {
	s.mu.Lock()
	s.authored[path] = true
	s.mu.Unlock()
}

// authoredIn returns the sorted-unique base names of authored files the
// command mentions (by absolute path or by base name).
func (s *fileState) authoredIn(command string) []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	seen := map[string]bool{}
	var names []string
	for p := range s.authored {
		base := filepath.Base(p)
		if (strings.Contains(command, p) || strings.Contains(command, base)) && !seen[base] {
			seen[base] = true
			names = append(names, base)
		}
	}
	sort.Strings(names)
	return names
}

// attempt counts one write or edit attempt on path and reports whether the
// edit-loop guard now refuses it.
func (s *fileState) attempt(path string) (count int, refused bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.edits[path]++
	return s.edits[path], s.edits[path] > editLoopLimit
}

func editLoopError(path string, n int) error {
	return fmt.Errorf("refusing attempt %d to change %s: you have already tried more than %d times in this session. "+
		"Stop editing it. Before touching it again, explain to the user what fundamentally different change or hypothesis you are testing and why the earlier attempts did not work",
		n, path, editLoopLimit)
}

var (
	churnStems  = regexp.MustCompile(`summary|finding|critical|session_|gaps?[-_]filled|quick[-_]reference|deliverables?|status[-_]report|progress[-_]report|wrap-up|completion[-_]report|implementation[-_]summary|final[-_]summary`)
	churnExempt = regexp.MustCompile(`^(readme|changelog|license|contributing)`)
)

// isChurnMarkdown identifies the throwaway "summary of what I did" documents a
// weak model writes instead of making progress.
func isChurnMarkdown(path string) bool {
	slash := filepath.ToSlash(path)
	segs := strings.Split(slash, "/")
	base := strings.ToLower(segs[len(segs)-1])
	if !strings.HasSuffix(base, ".md") || churnExempt.MatchString(base) {
		return false
	}
	for _, s := range segs[:len(segs)-1] {
		if strings.EqualFold(s, "docs") {
			return false
		}
	}
	return churnStems.MatchString(base)
}

// WriteFile creates or overwrites a file.
func WriteFile() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "write_file",
			Description: "Create or overwrite a file; use edit_file for small changes.",
			Parameters: obj(map[string]any{
				"path":    prop("string", "", "minLength", 1),
				"content": prop("string", ""),
			}, "path", "content"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			given, content := argStr(args, "path"), argStr(args, "content")
			path, err := ResolveForWrite(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			if err := secretGuard(given, path, "write"); err != nil {
				return "", nil, err
			}
			if err := privateGuard(given, path, "write"); err != nil {
				return "", nil, err
			}
			st := stateOf(tc)
			if isChurnMarkdown(relPath(tc.Root, path)) {
				st.mu.Lock()
				st.summaryWrites++
				over := st.summaryWrites > summaryWriteLimit
				st.mu.Unlock()
				if over {
					return "", nil, fmt.Errorf("refusing to write %s: you have written several summary or report documents already. "+
						"More of them is not progress. Write code or tests, or tell the user the result directly", given)
				}
			}
			if n, refused := st.attempt(path); refused {
				return "", nil, editLoopError(given, n)
			}
			known := st.isAuthored(path) || st.wasRead(path)
			if st.readInStep(path, tc.Step) && !st.isAuthored(path) {
				// she asked to read the file and to write it in one step, so the write was composed before the content was in her hands
				// (she wrote "<read_file_output>" and the new item, and the list was gone)
				return "", nil, fmt.Errorf("refused: you asked to read %s and to write it in the same step, so this content was written before you saw the file. Read the result first, then write the whole new content in your next step, or add to it with edit_file", given)
			}
			if toolOutputMarker.MatchString(content) {
				return "", nil, fmt.Errorf("refused: the content for %s holds a tool-output marker (such as <read_file_output>) instead of the file's text. Write the real text of the whole file, or add to it with edit_file", given)
			}
			st.markAuthored(path)

			md := map[string]any{"action": "write_file", "path": relPath(tc.Root, path), "content_length": len([]rune(content))}
			var before string
			info, statErr := os.Stat(path)
			switch {
			case statErr == nil && info.IsDir():
				return "", nil, fmt.Errorf("%s is a directory", given)
			case statErr == nil:
				data, err := os.ReadFile(path)
				if err != nil {
					return "", nil, fmt.Errorf("cannot read the existing %s: %v", given, err)
				}
				before = string(data)
				if isBinary(data) {
					return "", nil, fmt.Errorf("refused: %s is a binary file (an image, archive or program); writing text into it would break it. Leave it as it is and tell the user", given)
				}
				if strings.TrimSpace(before) != "" && !known && !strings.Contains(content, strings.TrimSpace(before)) {
					// writing replaces the whole file: content she has not seen would be lost (she "added" an item to a list by writing only the item)
					return "", nil, fmt.Errorf("refused: %s already has content you have not read in this task, and writing replaces all of it. Read it first, then write the whole new content; or change part of it with edit_file", given)
				}
				if strings.TrimSpace(before) != "" && strings.TrimSpace(content) == "" {
					// a model that is told there is no delete tool sometimes "deletes" by writing nothing: refuse, so the file survives
					return "", nil, fmt.Errorf("refused: writing nothing over %s would erase what is in it. There is no tool here to delete or empty files: leave it as it is and tell the user so", given)
				}
				md["before_exists"], md["before_sha256"] = true, sha256Hex(before)
				md["before_size"], md["before_preview"] = len(data), preview(before, 240)
			default:
				md["before_exists"], md["before_sha256"], md["before_size"], md["before_preview"] = false, nil, nil, nil
			}
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				return "", nil, fmt.Errorf("cannot create directories for %s: %v", given, err)
			}
			if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
				return "", nil, fmt.Errorf("cannot write %s: %v", given, err)
			}
			if err := verifyOnDisk(path, content); err != nil {
				return "", nil, fmt.Errorf("wrote %s but reading it back does not match: %v", given, err)
			}
			md["after_sha256"], md["after_size"], md["after_preview"] = sha256Hex(content), len(content), preview(content, 240)
			md["verified_on_disk"] = true
			return fmt.Sprintf("Wrote %s (%d characters). Read back from disk: it matches.", relPath(tc.Root, path), len([]rune(content))), md, nil
		},
	}
}

// EditFile replaces exact text in a file.
func EditFile() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "edit_file",
			Description: "Replace exact text in a file; old_text must match once unless replace_all.",
			Parameters: obj(map[string]any{
				"path":        prop("string", "", "minLength", 1),
				"old_text":    prop("string", "", "minLength", 1),
				"new_text":    prop("string", ""),
				"replace_all": prop("boolean", ""),
			}, "path", "old_text", "new_text"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			given, oldT, newT := argStr(args, "path"), argStr(args, "old_text"), argStr(args, "new_text")
			path, err := ResolveInRoot(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			if err := secretGuard(given, path, "edit"); err != nil {
				return "", nil, err
			}
			if err := privateGuard(given, path, "edit"); err != nil {
				return "", nil, err
			}
			st := stateOf(tc)
			// Counted before validation on purpose: failed edits are still attempts.
			if n, refused := st.attempt(path); refused {
				return "", nil, editLoopError(given, n)
			}
			st.markAuthored(path)
			info, err := os.Stat(path)
			if err != nil || !info.Mode().IsRegular() {
				return "", nil, fmt.Errorf("%s is not a regular file", given)
			}
			if info.Size() > maxWholeFileRead {
				return "", nil, fmt.Errorf("%s is too large to edit", given)
			}
			data, err := os.ReadFile(path)
			if err != nil {
				return "", nil, fmt.Errorf("cannot read %s: %v", given, err)
			}
			if isBinary(data) {
				return "", nil, fmt.Errorf("refused: %s is a binary file (an image, archive or program); editing it as text would break it. Leave it as it is and tell the user", given)
			}
			before := string(data)
			count := strings.Count(before, oldT)
			replaceAll := argBool(args, "replace_all", false)
			switch {
			case oldT == newT:
				return "", nil, fmt.Errorf("old_text and new_text are identical; nothing to change")
			case count == 0:
				return "", nil, fmt.Errorf("old_text was not found in %s. It must match exactly, including whitespace; read the file again and copy the text", given)
			case count > 1 && !replaceAll:
				return "", nil, fmt.Errorf("old_text matches %d places in %s. Add surrounding lines to make it unique, or set replace_all to change every one", count, given)
			}
			replaced := 1
			after := strings.Replace(before, oldT, newT, 1)
			if replaceAll {
				replaced = count
				after = strings.ReplaceAll(before, oldT, newT)
			}
			if strings.TrimSpace(before) != "" && strings.TrimSpace(after) == "" {
				return "", nil, fmt.Errorf("refused: that edit would leave %s empty, which erases it. There is no tool here to delete or empty files: leave it as it is and tell the user so", given)
			}
			if err := os.WriteFile(path, []byte(after), info.Mode().Perm()); err != nil {
				return "", nil, fmt.Errorf("cannot write %s: %v", given, err)
			}
			md := map[string]any{
				"action": "edit_file", "path": relPath(tc.Root, path),
				"before_sha256": sha256Hex(before), "after_sha256": sha256Hex(after),
				"before_size": len(before), "after_size": len(after),
				"before_preview": preview(before, 240), "after_preview": preview(after, 240),
				"old_text_preview": preview(oldT, 240), "new_text_preview": preview(newT, 240),
				"replaced_occurrences": replaced,
			}
			if err := verifyOnDisk(path, after); err != nil {
				return "", nil, fmt.Errorf("edited %s but reading it back does not match: %v", given, err)
			}
			md["verified_on_disk"] = true
			return fmt.Sprintf("Edited %s (%d replacement%s). Read back from disk: it matches.", relPath(tc.Root, path), replaced, plural(replaced)), md, nil
		},
	}
}

// verifyOnDisk reads a file back and checks it holds exactly what was meant to be written, so the agent never reports a change the
// disk does not have (a full disk, a filesystem that rewrites content, a file replaced meanwhile).
func verifyOnDisk(path, want string) error {
	got, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("cannot read it back: %v", err)
	}
	if string(got) != want {
		return fmt.Errorf("it holds %d bytes, expected %d", len(got), len(want))
	}
	return nil
}

func plural(n int) string {
	if n == 1 {
		return ""
	}
	return "s"
}

// isBinary: a NUL byte in the first 8 KiB, or text that is not valid UTF-8, means the file is not text and must not be rewritten as text.
func isBinary(data []byte) bool {
	head := data
	if len(head) > 8192 {
		head = head[:8192]
	}
	if bytes.IndexByte(head, 0) >= 0 {
		return true
	}
	return !utf8.Valid(head) && !utf8.Valid(data)
}
