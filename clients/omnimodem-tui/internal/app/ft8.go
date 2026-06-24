package app

import (
	"fmt"
	"time"
)

// ft8Seq is the standard FT8 QSO message ladder (WSJT-X Tx1..Tx5/Tx6).
type ft8Seq struct {
	myCall, myGrid string
	dxCall, dxGrid string
	report         int
	step           int
}

func newFT8Seq(myCall, myGrid string) *ft8Seq {
	return &ft8Seq{myCall: myCall, myGrid: myGrid, report: -10}
}

func (s *ft8Seq) target(call, grid string) { s.dxCall, s.dxGrid, s.step = call, grid, 0 }
func (s *ft8Seq) advance()                 { s.step++ }

// current returns the message for the current ladder step.
func (s *ft8Seq) current() string {
	switch s.step {
	case 0:
		return fmt.Sprintf("%s %s %s", s.dxCall, s.myCall, s.myGrid) // Tx1: grid
	case 1:
		return fmt.Sprintf("%s %s %+d", s.dxCall, s.myCall, s.report) // Tx2: report
	case 2:
		return fmt.Sprintf("%s %s R%+d", s.dxCall, s.myCall, s.report) // Tx3: R-report
	case 3:
		return fmt.Sprintf("%s %s RR73", s.dxCall, s.myCall) // Tx4
	default:
		return fmt.Sprintf("%s %s 73", s.dxCall, s.myCall) // Tx5
	}
}

// cq is the calling message (Tx6).
func (s *ft8Seq) cq() string { return fmt.Sprintf("CQ %s %s", s.myCall, s.myGrid) }

// finished reports whether the ladder has reached RR73/73 (→ prompt a log entry).
func (s *ft8Seq) finished() bool { return s.step >= 3 }

// slotPosition returns seconds into the current 15 s FT8 slot (0..15).
func slotPosition(at time.Time) float64 {
	return float64(at.UTC().Second()%15) + float64(at.Nanosecond())/1e9
}
