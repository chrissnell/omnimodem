package app

import (
	"fmt"
	"math"
	"time"
)

// ft8Seq is the standard FT8 QSO message ladder (WSJT-X Tx1..Tx5/Tx6).
type ft8Seq struct {
	myCall, myGrid string
	dxCall, dxGrid string
	report         int
	step           int
}

// ladderRR73Step is the ladder step that sends RR73 — i.e. the point at which
// the QSO is complete and should be logged exactly once.
const ladderRR73Step = 3

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

// slotPosition returns seconds into the current T/R slot of length `period`
// (0..period), anchored to the UTC epoch. period dividing 60 (15 s FT8, 7.5 s
// FT4, 60 s JT65/JT9) aligns to UTC minute boundaries; 120 s (WSPR) aligns to
// even minutes — matching WSJT-X. `period <= 0` degenerates to 0.
func slotPosition(at time.Time, period float64) float64 {
	if period <= 0 {
		return 0
	}
	return math.Mod(float64(at.UTC().UnixNano())/1e9, period)
}
