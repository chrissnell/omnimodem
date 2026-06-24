package client

import "testing"

func TestDialTarget(t *testing.T) {
	cases := map[string]string{
		"/run/omnimodem.sock": "unix:///run/omnimodem.sock",
		"127.0.0.1:9000":      "dns:///127.0.0.1:9000",
	}
	for in, want := range cases {
		if got := dialTarget(in); got != want {
			t.Fatalf("dialTarget(%q) = %q, want %q", in, got, want)
		}
	}
}
