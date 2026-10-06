// Run from the SSM Go module: go run /path/to/decode_card_bundles.go INPUT OUTPUT.
// Uses the game's cached bundle filename to decrypt the Sirius header.
package main

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/kvarenzn/ssm/k"
)

func main() {
	if len(os.Args) != 3 {
		panic("usage: decode_card_bundles INPUT OUTPUT")
	}
	inDir, outDir := os.Args[1], os.Args[2]
	if err := os.MkdirAll(outDir, 0755); err != nil {
		panic(err)
	}
	n, failures := 0, 0
	err := filepath.Walk(inDir, func(p string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		name := strings.ToLower(info.Name())
		if info.IsDir() || (!strings.Contains(name, "membercard") && !strings.Contains(name, "supportcard")) || !strings.HasSuffix(name, ".bundle") {
			return nil
		}
		b, e := os.ReadFile(p)
		if e != nil {
			failures++
			fmt.Fprintln(os.Stderr, p, e)
			return nil
		}
		var r io.Reader = bytes.NewReader(b)
		if !bytes.HasPrefix(b, []byte("UnityFS\x00")) {
			r, e = k.NewSiriusAssetFile(r, k.SiriusFilename(filepath.Base(p)))
			if e != nil {
				failures++
				fmt.Fprintln(os.Stderr, p, e)
				return nil
			}
		}
		d, e := io.ReadAll(r)
		if e == nil {
			e = os.WriteFile(filepath.Join(outDir, filepath.Base(p)), d, 0644)
		}
		if e != nil {
			failures++
			fmt.Fprintln(os.Stderr, p, e)
			return nil
		}
		n++
		return nil
	})
	if err != nil {
		panic(err)
	}
	fmt.Printf("decoded %d failures %d\n", n, failures)
	if failures > 0 {
		os.Exit(1)
	}
}
