// throb_dump.cxx — fldigi Throb / ThrobX golden-vector extractor.
//
// Emits the bit-domain reference for the Throb modem: both tone-pair tables
// (0-based), both character sets, both frequency tables, and — for a fixed
// message per family — the exact (tone1,tone2) pair sequence fldigi's
// tx_process()/send() put on the wire (reverse = false, pre-offset), including
// the 4-symbol idle preamble and the ThrobX idle/space flip.
//
// Throb's whole wire behaviour is table-driven and FLTK-free at the data level,
// but throb.cxx's tx_process() is a `modem` subclass method entangled with the
// FLTK/config runtime (get_tx_char/ModulateXmtr/progdefaults), so it cannot be
// linked standalone the way pskvaricode.cxx / dominovar.cxx can. This driver
// therefore transcribes the reference *data* verbatim (the four tables below are
// byte-for-byte copies of throb.cxx, cited) and re-expresses only tx_process()'s
// char->symbol *selection* logic, each branch cited to throb.cxx. The tables are
// the source of truth; omnimodem's modes::throb must reproduce this JSON exactly.
//
// Provenance:
//   upstream: fldigi 4.1.23 (commit 61b97f413)
//   source:   src/throb/throb.cxx (tables :729-944, tx framing :582-721,
//             reset/flip :96-130, rx decode :357-417), src/include/throb.h:34-46
//   build:    scratch/refvectors/build_throb.sh
//
// Output: one JSON object, consumed by crates/dsp/tests/vectors/throb.json.

#include <cstdio>
#include <cctype>
#include <cstring>
#include <vector>

// ---- verbatim from throb.cxx:729-775 (ThrobTonePairs[45][2], 1-based) -------
static const int ThrobTonePairs[45][2] = {
	{5,5},{4,5},{1,2},{1,3},{1,4},{4,6},{1,5},{1,6},{1,7},{3,7},
	{1,8},{2,3},{2,4},{2,8},{2,5},{5,6},{2,6},{2,9},{3,4},{3,5},
	{1,9},{3,6},{8,9},{3,8},{3,3},{2,2},{1,1},{3,9},{4,7},{4,8},
	{4,9},{5,7},{5,8},{5,9},{6,7},{6,8},{6,9},{7,8},{7,9},{8,8},
	{7,7},{6,6},{4,4},{9,9},{2,7}
};
// ---- verbatim from throb.cxx:777-833 (ThrobXTonePairs[55][2], 1-based) -------
static const int ThrobXTonePairs[55][2] = {
	{6,11},{1,6},{2,6},{2,5},{2,7},{2,8},{5,6},{2,9},{2,10},{4,8},
	{4,6},{2,11},{3,4},{3,5},{3,6},{6,9},{6,10},{3,7},{3,8},{3,9},
	{6,8},{6,7},{3,10},{3,11},{4,5},{4,7},{4,9},{4,10},{1,2},{1,3},
	{1,4},{1,5},{1,7},{1,8},{1,9},{1,10},{2,3},{2,4},{4,11},{5,7},
	{5,8},{5,9},{5,10},{5,11},{7,8},{7,9},{7,10},{7,11},{8,9},{8,10},
	{8,11},{9,10},{9,11},{10,11},{1,11}
};
// ---- verbatim from throb.cxx:835-881 (ThrobCharSet[45]) ----------------------
static const unsigned char ThrobCharSet[45] = {
	'\0','A','B','C','D','\0','F','G','H','I','J','K','L','M','N','O','P',
	'Q','R','S','T','U','V','W','X','Y','Z','1','2','3','4','5','6','7','8',
	'9','0',',','.','\'','/',')','(','E',' '
};
// ---- verbatim from throb.cxx:883-939 (ThrobXCharSet[55]) ---------------------
static const unsigned char ThrobXCharSet[55] = {
	'\0',' ','A','B','C','D','E','F','G','H','I','J','K','L','M','N','O','P',
	'Q','R','S','T','U','V','W','X','Y','Z','1','2','3','4','5','6','7','8','9',
	'0',',','.','\'','/',')','(','#','"','+','-',';',':','?','!','@','=','\n'
};
// ---- verbatim from throb.cxx:941-944 (frequency tables, Hz offsets) ----------
static const double ThrobToneFreqsNar[9]  = {-32,-24,-16,-8,0,8,16,24,32};
static const double ThrobToneFreqsWid[9]  = {-64,-48,-32,-16,0,16,32,48,64};
static const double ThrobXToneFreqsNar[11] = {-39.0625,-31.25,-23.4375,-15.625,-7.8125,0,7.8125,15.625,23.4375,31.25,39.0625};
static const double ThrobXToneFreqsWid[11] = {-78.125,-62.5,-46.875,-31.25,-15.625,0,15.625,31.25,46.875,62.5,78.125};

// Emitted (tone1,tone2) pairs, 0-based (send() subtracts 1; reverse=false).
struct Pairs { std::vector<int> t1, t2; };

static void emit(Pairs &p, const int table[][2], int sym) {
	p.t1.push_back(table[sym][0] - 1);
	p.t2.push_back(table[sym][1] - 1);
}

// Replicates throb.cxx tx_process() char->symbol selection for regular Throb
// (throb.cxx:648-717) + the 4-idle preamble (:625-630). num_chars = 45.
static Pairs throb_seq(const char *msg) {
	Pairs p;
	const int idlesym = 0;              // reset_syms(): idle=0, space=44 (:125-129)
	for (int i = 0; i < 4; i++) emit(p, ThrobTonePairs, idlesym); // preamble (:625-630)
	for (const char *s = msg; *s; ++s) {
		int c = (unsigned char)*s;
		// special shifted chars (throb.cxx:653-682)
		if (c == '?') { emit(p, ThrobTonePairs, 5); emit(p, ThrobTonePairs, 20); continue; }
		if (c == '@') { emit(p, ThrobTonePairs, 5); emit(p, ThrobTonePairs, 13); continue; }
		if (c == '-') { emit(p, ThrobTonePairs, 5); emit(p, ThrobTonePairs, 9);  continue; }
		if (c == '\r') continue;
		if (c == '\n') { emit(p, ThrobTonePairs, 5); emit(p, ThrobTonePairs, 0);  continue; }
		if (islower(c)) c = toupper(c);                              // (:690-691)
		int sym = -1;                                                // (:698-701)
		for (int i = 0; i < 45; i++) if (c == ThrobCharSet[i]) sym = i;
		if (sym == -1) c = ' ';                                      // (:710)
		if (c == ' ') sym = 44;                                      // spacesym=44 (:712-715)
		emit(p, ThrobTonePairs, sym);                                // (:717)
	}
	return p;
}

// Replicates tx_process() for ThrobX (no shift; idle/space swap 0<->1 via
// flip_syms, throb.cxx:96-114/703-715). num_chars = 55. reset_syms: idle=0,space=1.
static Pairs throbx_seq(const char *msg) {
	Pairs p;
	int idlesym = 0, spacesym = 1;
	auto flip = [&]() { if (idlesym == 0) { idlesym = 1; spacesym = 0; }
	                    else { idlesym = 0; spacesym = 1; } };
	for (int i = 0; i < 4; i++) { emit(p, ThrobXTonePairs, idlesym); flip(); } // preamble (:625-630)
	for (const char *s = msg; *s; ++s) {
		int c = (unsigned char)*s;
		if (islower(c)) c = toupper(c);
		int sym = -1;
		for (int i = 0; i < 55; i++) if (c == ThrobXCharSet[i]) sym = i;
		if (sym == -1) c = ' ';
		if (c == ' ') { sym = spacesym; flip(); }                    // (:712-715)
		emit(p, ThrobXTonePairs, sym);
	}
	return p;
}

static void print_pairs(const char *key, const char *msg, const Pairs &p) {
	printf("{\"mode\":\"%s\",\"text\":\"%s\",\"tones\":[", key, msg);
	for (size_t i = 0; i < p.t1.size(); ++i)
		printf("%s[%d,%d]", i ? "," : "", p.t1[i], p.t2[i]);
	printf("]}");
}

int main() {
	printf("{\"_meta\":{\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\",");
	printf("\"files\":[\"src/throb/throb.cxx (tables :729-944, tx :582-721, reset/flip :96-130)\",\"src/include/throb.h:34-46\"],");
	printf("\"cmd\":\"bash scratch/refvectors/build_throb.sh\",");
	printf("\"note\":\"tone-pair tables 0-based; charsets verbatim; freqs Hz offset. messages[]: emitted (tone1,tone2) sequence from tx_process incl 4-idle preamble, reverse=false. throb family = Throb1/2/4; throbx family = ThrobX1/2/4 (tone set differs only in Hz spacing, not indices).\"},");

	// tables (0-based tone pairs)
	printf("\"throb_tonepairs\":[");
	for (int i = 0; i < 45; i++) printf("%s[%d,%d]", i?",":"", ThrobTonePairs[i][0]-1, ThrobTonePairs[i][1]-1);
	printf("],\"throbx_tonepairs\":[");
	for (int i = 0; i < 55; i++) printf("%s[%d,%d]", i?",":"", ThrobXTonePairs[i][0]-1, ThrobXTonePairs[i][1]-1);
	printf("],");

	// char sets (as decimal byte arrays; 0 = non-printing idle/shift)
	printf("\"throb_charset\":[");
	for (int i = 0; i < 45; i++) printf("%s%d", i?",":"", ThrobCharSet[i]);
	printf("],\"throbx_charset\":[");
	for (int i = 0; i < 55; i++) printf("%s%d", i?",":"", ThrobXCharSet[i]);
	printf("],");

	// freq tables
	auto fr = [](const char *k, const double *a, int n, const char *tail) {
		printf("\"%s\":[", k);
		for (int i = 0; i < n; i++) printf("%s%g", i?",":"", a[i]);
		printf("]%s", tail);
	};
	fr("throb_freqs_nar", ThrobToneFreqsNar, 9, ",");
	fr("throb_freqs_wid", ThrobToneFreqsWid, 9, ",");
	fr("throbx_freqs_nar", ThrobXToneFreqsNar, 11, ",");
	fr("throbx_freqs_wid", ThrobXToneFreqsWid, 11, ",");

	// message tone sequences
	const char *m1 = "CQ DE K1ABC";
	const char *m2 = "R U THERE? 73";
	printf("\"messages\":[");
	print_pairs("throb", m1, throb_seq(m1));  printf(",");
	print_pairs("throb", m2, throb_seq(m2));  printf(",");
	print_pairs("throbx", m1, throbx_seq(m1)); printf(",");
	print_pairs("throbx", m2, throbx_seq(m2));
	printf("]}\n");
	return 0;
}
