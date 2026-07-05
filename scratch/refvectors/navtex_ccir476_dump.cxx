// navtex_ccir476_dump.cxx — fldigi NAVTEX / SITOR-B CCIR-476 golden extractor.
//
// Emits the bit-exact reference for the CCIR-476 (SITOR) code and its FEC-B
// time-diversity framing:
//   1. the full 128-entry code->letter and code->figure tables + the 4-of-7
//      validity mask (CCIR476::check_bits);
//   2. per fixed message: the char->code sequence (`encode`), the FEC-B
//      diversity stream (`create_fec`), and the LSB-first bit stream that
//      `send_string` clocks onto the wire.
//
// The CCIR476 class + tables + `encode`/`create_fec` are transcribed VERBATIM
// with cites from navtex.cxx below. They live inside the fldigi modem class
// (navtex.cxx), which cannot link standalone (FLTK / fftfilt / modem runtime),
// so — exactly as scratch/refvectors/dominoex_varicode_dump.cxx does for the
// DominoEX varicode — the self-contained table-driven pieces are copied here
// unmodified. The Rust port (crates/dsp/src/fec/ccir476.rs) must reproduce this
// output byte-for-byte.
//
// Provenance:
//   upstream: w1hkj/fldigi 4.1.23 (commit 61b97f413)
//   source:   src/navtex/navtex.cxx:465-592 (tables + CCIR476 class),
//             src/navtex/navtex.cxx:1711-1743 (create_fec + encode)
//   build:    scratch/refvectors/build_navtex_ccir476.sh
//
// Output: JSON to stdout, consumed by crates/dsp/tests/vectors/navtex_ccir476.json.

#include <cstdio>
#include <cstring>
#include <string>
#include <cctype>

// ---- verbatim from navtex.cxx:465-495 ----
static const unsigned char code_to_ltrs[128] = {
	//0 1   2   3   4   5   6   7   8   9   a   b   c   d   e   f
	'_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', // 0
	'_', '_', '_', '_', '_', '_', '_', 'J', '_', '_', '_', 'F', '_', 'C', 'K', '_', // 1
	'_', '_', '_', '_', '_', '_', '_', 'W', '_', '_', '_', 'Y', '_', 'P', 'Q', '_', // 2
	'_', '_', '_', '_', '_', 'G', '_', '_', '_', 'M', 'X', '_', 'V', '_', '_', '_', // 3
	'_', '_', '_', '_', '_', '_', '_', 'A', '_', '_', '_', 'S', '_', 'I', 'U', '_', // 4
	'_', '_', '_', 'D', '_', 'R', 'E', '_', '_', 'N', '_', '_', ' ', '_', '_', '_', // 5
	'_', '_', '_', 'Z', '_', 'L', '_', '_', '_', 'H', '_', '_', '\n', '_', '_', '_', // 6
	'_', 'O', 'B', '_', 'T', '_', '_', '_', '\r', '_', '_', '_', '_', '_', '_', '_' // 7
};

static const unsigned char code_to_figs[128] = {
	//0 1   2   3   4   5   6   7   8   9   a   b   c   d   e   f
	'_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', '_', // 0
	'_', '_', '_', '_', '_', '_', '_', '\'', '_', '_', '_', '!', '_', ':', '(', '_', // 1
	'_', '_', '_', '_', '_', '_', '_', '2', '_', '_', '_', '6', '_', '0', '1', '_', // 2
	'_', '_', '_', '_', '_', '&', '_', '_', '_', '.', '/', '_', ';', '_', '_', '_', // 3
	'_', '_', '_', '_', '_', '_', '_', '-', '_', '_', '_', '\07', '_', '8', '7', '_', // 4
	'_', '_', '_', '$', '_', '4', '3', '_', '_', ',', '_', '_', ' ', '_', '_', '_', // 5
	'_', '_', '_', '"', '_', ')', '_', '_', '_', '#', '_', '_', '\n', '_', '_', '_', // 6
	'_', '9', '?', '_', '5', '_', '_', '_', '\r', '_', '_', '_', '_', '_', '_', '_' // 7
};

static const int code_ltrs = 0x5a;
static const int code_figs = 0x36;
static const int code_alpha = 0x0f;
static const int code_beta = 0x33;
static const int code_char32 = 0x6a;
static const int code_rep = 0x66;

// ---- verbatim from navtex.cxx:497-592 (CCIR476 class) ----
class CCIR476 {
	unsigned char m_ltrs_to_code[128];
	unsigned char m_figs_to_code[128];
	bool m_valid_codes[128];
public:
	CCIR476() {
		memset( m_ltrs_to_code, 0, 128 );
		memset( m_figs_to_code, 0, 128 );
		for( size_t i = 0; i < 128; ++i ) m_valid_codes[i] = false ;
		for (int code = 0; code < 128; code++) {
			if (check_bits(code)) {
				m_valid_codes[code] = true;
				unsigned char figv = code_to_figs[code];
				unsigned char ltrv = code_to_ltrs[code];
				if ( figv != '_') {
					m_figs_to_code[figv] = code;
				}
				if ( ltrv != '_') {
					m_ltrs_to_code[ltrv] = code;
				}
			}
		}
	}

	void char_to_code(std::string & str, int ch, bool & ex_shift) const {
		ch = toupper(ch);
		if (ex_shift && m_figs_to_code[ch] != '\0') {
			str.push_back(  m_figs_to_code[ch] );
		}
		else if (!ex_shift && m_ltrs_to_code[ch] != '\0') {
			str.push_back( m_ltrs_to_code[ch] );
		}
		else if (m_figs_to_code[ch] != '\0') {
			ex_shift = true;
			str.push_back( code_figs );
			str.push_back( m_figs_to_code[ch] );
		}
		else if (m_ltrs_to_code[ch] != '\0') {
			ex_shift = false;
			str.push_back( code_ltrs );
			str.push_back( m_ltrs_to_code[ch] );
		}
	}

	int code_to_char(int code, bool shift) const {
		const unsigned char * target = (shift) ? code_to_figs : code_to_ltrs;
		if (target[code] != '_') {
			return target[code];
		}
		return -code;
	}

	static bool check_bits(int v) {
		int bc = 0;
		while (v != 0) {
			bc++;
			v &= v - 1;
		}
		return bc == 4;
	}
};

// ---- verbatim from navtex.cxx:1711-1743 (create_fec + encode) ----
static CCIR476 g_ccir476;

static std::string create_fec( const std::string & str )
{
	std::string res ;
	const size_t sz = str.size();
	static const size_t offset = 2 ;
	for( size_t i = 0 ; i < offset ; ++i ) {
		res.push_back( code_rep );
		res.push_back( code_alpha );
	}
	for ( size_t i = 0; i < sz; ++i ) {
		res.push_back( str[i] );
		res.push_back( i >= offset ? str[ i - offset ] : code_alpha );
	}
	for( size_t i = 0 ; i < offset ; ++i ) {
		res.push_back( code_char32 );
		res.push_back( str[ sz - offset + i ] );
	}
	return res;
}

static std::string encode( const std::string & str )
{
	std::string res ;
	bool shift = false ;
	for ( size_t i = 0, sz = str.size(); i < sz; ++ i ) {
		g_ccir476.char_to_code(res, str[i], shift );
	}
	return res;
}

static void dump_msg(const char *label, const char *msg) {
	std::string enc = encode(msg);
	std::string fec = create_fec(enc);
	printf("{\"label\":\"%s\",\"msg\":\"%s\",\"codes\":[", label, msg);
	for (size_t i = 0; i < enc.size(); i++)
		printf(i ? ",%d" : "%d", (unsigned char)enc[i]);
	printf("],\"fec\":[");
	for (size_t i = 0; i < fec.size(); i++)
		printf(i ? ",%d" : "%d", (unsigned char)fec[i]);
	printf("],\"bits\":\"");
	// send_string clocks each 7-bit code LSB-first (navtex.cxx:1798-1802).
	for (size_t i = 0; i < fec.size(); i++) {
		unsigned char c = (unsigned char)fec[i];
		for (int j = 0; j < 7; j++, c >>= 1)
			printf("%d", c & 1);
	}
	printf("\"}");
}

int main() {
	printf("{\"_meta\":{\"upstream\":\"w1hkj/fldigi 4.1.23 @ 61b97f413\",");
	printf("\"files\":[\"src/navtex/navtex.cxx:465-592 (CCIR-476 tables + class, verbatim)\",");
	printf("\"src/navtex/navtex.cxx:1711-1743 (create_fec + encode, verbatim)\"],");
	printf("\"driver\":\"scratch/refvectors/build_navtex_ccir476.sh\",");
	printf("\"note\":\"codes/fec are 7-bit CCIR-476 codewords (4-of-7). bits are LSB-first per code, as send_string clocks them. bit-exact.\"},\n");

	// Full code tables + 4-of-7 validity.
	printf("\"table\":[");
	for (int c = 0; c < 128; c++) {
		unsigned char lt = code_to_ltrs[c];
		unsigned char fg = code_to_figs[c];
		printf("%s{\"code\":%d,\"valid\":%s,\"ltrs\":%d,\"figs\":%d}",
		       c ? "," : "", c, CCIR476::check_bits(c) ? "true" : "false",
		       (int)lt, (int)fg);
	}
	printf("],\n\"messages\":[\n");
	dump_msg("cq", "CQ DE K1ABC");
	printf(",\n");
	dump_msg("nautical", "NAUTICAL");
	printf(",\n");
	dump_msg("mixed", "SECURITE 12 SHIPS 3.5M");
	printf("\n]}\n");
	return 0;
}
