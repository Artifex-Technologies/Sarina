package agent

import (
	"os"
	"testing"
)

// The installed lessons list (/usr/local/share/sarina/lessons.md) must not change what these tests see; a test that wants
// lessons sets SARINA_LESSONS_FILE itself.
func TestMain(m *testing.M) {
	os.Setenv("SARINA_LESSONS_FILE", "off")
	os.Exit(m.Run())
}
