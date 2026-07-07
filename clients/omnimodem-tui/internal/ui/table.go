package ui

import (
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/lipgloss/table"
)

// Column is one column of a Table: a header label and a fixed content width
// (excluding the one-space padding Table adds on each side).
type Column struct {
	Title string
	Width int
}

// Table renders a bordered data table in the app palette with one row
// highlighted (the cursor). It wraps lipgloss/table so every list in the TUI —
// device pickers now, channels and spots later — shares one look: a rounded grey
// frame, a bold yellow header, white body rows, and a blue selection bar on the
// highlighted row. Columns are fixed-width (cells are truncated to fit), which
// keeps the frame square; selected is a 0-based data-row index (< 0 highlights
// nothing).
//
// Column widths are applied through the StyleFunc rather than Table.Width(),
// which is deliberate: lipgloss/table drops the right border when an explicit
// Width is combined with BorderColumn(false).
func Table(cols []Column, rows [][]string, selected int) string {
	return renderTable(cols, rows, selected, true)
}

// TableInset renders the same table with no outer frame (only the header rule),
// for dropping inside a Card or other container that already supplies the box.
func TableInset(cols []Column, rows [][]string, selected int) string {
	return renderTable(cols, rows, selected, false)
}

func renderTable(cols []Column, rows [][]string, selected int, bordered bool) string {
	headers := make([]string, len(cols))
	for i, c := range cols {
		headers[i] = c.Title
	}
	// Truncate each cell to its column width so a long device label can't stretch
	// the frame past its budget.
	trimmed := make([][]string, len(rows))
	for r, row := range rows {
		cells := make([]string, len(cols))
		for c := range cols {
			if c < len(row) {
				cells[c] = truncate(row[c], cols[c].Width)
			}
		}
		trimmed[r] = cells
	}

	base := lipgloss.NewStyle().Background(ColorPanel).Foreground(ColorFg).Padding(0, 1)
	colStyle := func(c int) lipgloss.Style {
		w := 4
		if c < len(cols) {
			w = cols[c].Width
		}
		// lipgloss Width() is the block width INCLUDING padding, so the content
		// area is w only when the block is w+2 (Padding(0,1) each side). MaxWidth
		// clips the whole block so a stray wide glyph can't grow the column.
		return base.Width(w + 2).MaxWidth(w + 2)
	}
	t := table.New().
		Border(lipgloss.RoundedBorder()).
		BorderStyle(lipgloss.NewStyle().Foreground(ColorDim).Background(ColorPanel)).
		BorderRow(false).
		BorderColumn(false).
		Headers(headers...).
		Rows(trimmed...).
		StyleFunc(func(row, col int) lipgloss.Style {
			switch {
			case row == table.HeaderRow:
				return colStyle(col).Foreground(ColorTitle).Bold(true)
			case row == selected:
				return colStyle(col).Foreground(ColorFg).Background(ColorSel).Bold(true)
			default:
				return colStyle(col)
			}
		})
	if !bordered {
		t = t.BorderTop(false).BorderBottom(false).BorderLeft(false).BorderRight(false)
	}
	return t.Render()
}

// TableWidth is the outer width a Table with the given columns renders to, so
// callers can size a surrounding modal to hug it (border + 1-col padding each
// side, per column, plus the two frame borders).
func TableWidth(cols []Column) int {
	w := 2 // left+right frame border
	for _, c := range cols {
		w += c.Width + 2 // content + Padding(0,1)
	}
	return w
}
