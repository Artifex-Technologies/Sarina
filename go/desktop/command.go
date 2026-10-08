package desktop

import (
	"flag"
	"fmt"
	"io"
	"path/filepath"

	"github.com/Artifex-Technologies/Sarina/go/analysiscache"
	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/helpers"
)

// HelperScorer builds a semantic Scorer for a model directory by starting the
// Python helper (python/sarina_helpers; ONNX embeddings are Python-only). It
// fails, with the reason, when the helper is not installed or cannot load the
// model, or when its prototype set differs from this build's (cached scores
// would then mean something else). Tests replace this variable.
var HelperScorer = func(modelDir string) (Scorer, error) {
	srv, err := helpers.StartServer(modelDir)
	if err != nil {
		return nil, fmt.Errorf("semantic classification needs the Python helper (%s): %v", helpers.InstallHint, err)
	}
	if srv.Prototypes != PrototypesFingerprint() {
		srv.Close()
		return nil, fmt.Errorf("the Python helper's category prototypes differ from this build's; update python/sarina_helpers")
	}
	return &helperScorer{srv}, nil
}

type helperScorer struct{ srv *helpers.Server }

func (h *helperScorer) Score(text string) (string, float64, error) { return h.srv.Classify(text) }
func (h *helperScorer) Close() error                               { return h.srv.Close() }

type deps struct {
	entries  func() []DesktopEntry
	packages func() []InstalledPackage
}

func defaultDeps() deps {
	return deps{
		entries:  func() []DesktopEntry { return ScanDesktopEntries(ApplicationDirs()) },
		packages: func() []InstalledPackage { return QueryInstalledPackages("pacman", nil) },
	}
}

func init() {
	cli.Register("desktop-audit", "audit and repair the application menu (report only unless --apply)",
		func(env *cli.Env, args []string) int { return runAudit(env, args, defaultDeps()) })
}

func runAudit(env *cli.Env, args []string, d deps) int {
	fs := flag.NewFlagSet("desktop-audit", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	modelDir := fs.String("model-dir", "", "directory of an embedding model for semantic classification")
	threshold := fs.Float64("similarity-threshold", 0.35, "minimum similarity for a semantic match")
	inclDeps := fs.Bool("include-dependencies", false, "also consider packages installed as dependencies")
	inclLibs := fs.Bool("include-libraries", false, "also consider library-shaped packages")
	apply := fs.Bool("apply", false, "write the proposed entries")
	minConf := fs.Float64("min-confidence", 0.0, "with --apply, skip proposals below this confidence")
	target := fs.String("target-dir", "", "where entries are written (default: the user application directory)")
	removeGen := fs.Bool("remove-generated", false, "delete previously generated entries and exit")
	noCache := fs.Bool("no-cache", false, "do not cache semantic classification results")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	out := env.Stdout

	if *removeGen {
		removed := RemoveGenerated(*target)
		for _, p := range removed {
			fmt.Fprintf(out, "removed %s\n", p)
		}
		fmt.Fprintf(out, "%d generated entries removed.\n", len(removed))
		return 0
	}

	opt := AuditOptions{IncludeDependencies: *inclDeps, IncludeLibraries: *inclLibs}
	if *modelDir != "" {
		scorer, err := HelperScorer(*modelDir)
		if err != nil {
			fmt.Fprintf(out, "semantic inference unavailable: %v\n", err)
			opt.UnavailableReason = err.Error()
		} else {
			if c, ok := scorer.(io.Closer); ok {
				defer c.Close()
			}
			sc := &SemanticClassifier{Scorer: scorer, Threshold: *threshold}
			if !*noCache {
				sc.Cache = DefaultClassificationCache(cachePath(env))
			}
			opt.Classifier = sc
		}
	}

	audit := AuditMenu(d.entries(), d.packages(), opt)
	fmt.Fprintln(out, audit.AsText())

	if !*apply {
		if len(audit.Proposals) > 0 {
			fmt.Fprintln(out)
			fmt.Fprintln(out, "Nothing was written. Re-run with --apply to write these entries.")
		}
		return 0
	}
	written, err := ApplyProposals(audit.Proposals, *target, *minConf)
	for _, p := range written {
		fmt.Fprintf(out, "wrote %s\n", p)
	}
	if err != nil {
		fmt.Fprintf(env.Stderr, "desktop-audit: %v\n", err)
	}
	fmt.Fprintf(out, "%d entries written. Undo with --remove-generated.\n", len(written))
	return 0
}

func cachePath(env *cli.Env) string {
	base := ""
	if env.Home != "" {
		base = filepath.Join(env.Home, ".sarina", "cache")
	} else {
		base = analysiscache.DefaultDir()
	}
	return filepath.Join(base, "desktop_classify.json")
}
