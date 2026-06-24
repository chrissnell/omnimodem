package app

import "time"

type qsoEntry struct {
	utc  time.Time
	call string
	grid string
	rst  string
}

// qsoLog is a local append-only QSO log. The operate screen adds an entry when
// an FT8 exchange reaches 73/RR73 (design §6.4).
type qsoLog struct{ entries []qsoEntry }

func (l *qsoLog) add(call, grid, rst string) {
	l.entries = append(l.entries, qsoEntry{utc: time.Now().UTC(), call: call, grid: grid, rst: rst})
}
