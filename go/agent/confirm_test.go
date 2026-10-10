package agent

import (
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func TestSetConfirmAddsOnlyItsOwnRuntimeKey(t *testing.T) {
	dir := t.TempDir()
	a, err := New(Options{Model: config.ModelConfig{Model: "test", BaseURL: "http://127.0.0.1:1/v1"}, Runtime: config.DefaultRuntime(dir), Home: dir})
	if err != nil {
		t.Fatal(err)
	}
	a.toolCtx.Runtimes["attached_later"] = 42 // what memory and other runtimes do after construction
	if _, ok := a.toolCtx.Runtimes[tools.RuntimeOperatorConfirm]; ok {
		t.Fatal("no confirm channel was installed, yet the key is present")
	}
	a.SetConfirm(func(string) bool { return true })
	rt := a.toolCtx.Runtimes
	if _, ok := rt[tools.RuntimeOperatorConfirm].(func(string) bool); !ok {
		t.Fatal("the confirm channel was not installed")
	}
	if rt["attached_later"] != 42 || rt[tools.RuntimePlanActive] == nil {
		t.Fatalf("SetConfirm dropped other runtimes: %v", rt)
	}
	// A rebuilt tool context (workspace change) keeps the channel.
	if err := a.rebuildTools(dir); err != nil {
		t.Fatal(err)
	}
	if _, ok := a.toolCtx.Runtimes[tools.RuntimeOperatorConfirm]; !ok {
		t.Fatal("rebuilding the tool context lost the confirm channel")
	}
	a.SetConfirm(nil)
	if _, ok := a.toolCtx.Runtimes[tools.RuntimeOperatorConfirm]; ok {
		t.Fatal("SetConfirm(nil) left the channel installed")
	}
}
