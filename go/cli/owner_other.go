//go:build !unix

package cli

func ownedByOther(string) (int, bool) { return 0, false }
