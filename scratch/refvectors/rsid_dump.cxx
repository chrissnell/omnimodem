// rsid_dump.cxx — fldigi RSID (Reed-Solomon Identifier) golden-vector extractor.
//
// Emits, for every entry in fldigi's two RSID ID tables, the 15 Reed-Solomon
// symbols (4-bit each) that cRsId::Encode() produces for that entry's rsid code.
// These symbol sequences are the *bit-exact* on-air tone-index sequence for the
// RSID burst, and are the reference KAT for the Rust port's RS encoder + ID
// table (crates/dsp/src/frontend/rsid.rs).
//
// The RS(15,3)-over-GF(16) encoder (Squares/indices tables + Encode) is a pure
// integer function; it is transcribed here verbatim with cites, mirroring the
// feldhell_dump.cxx precedent (fldigi's runtime object cannot link standalone).
// The two ID tables (RSID_LIST / RSID_LIST2) are the reference *data* and are
// copied verbatim from rsid_defs.cxx; only fldigi's internal MODE_* tokens
// (which never go on the air) are captured as strings rather than resolved to
// the fldigi trx_mode enum, so no fldigi headers are needed.
//
// Provenance:
//   upstream: w1hkj/fldigi (checked out at ../fldigi)
//   source:   src/rsid/rsid.cxx:58-79   (Squares[256], indices[12] — verbatim)
//             src/rsid/rsid.cxx:184-196  (Encode — verbatim)
//             src/rsid/rsid_defs.cxx:29-217  (RSID_LIST  primary ID table)
//             src/rsid/rsid_defs.cxx:232-306 (RSID_LIST2 extended ID table)
//             src/include/rsid.h:60 (RSID_NSYMBOLS = 15)
//   build:    scratch/refvectors/build_rsid.sh
//
// Output: JSON-lines to stdout, consumed by crates/dsp/tests/vectors/rsid.json.

#include <cstdio>

#define RSID_NSYMBOLS 15 // ref: rsid.h:60

// ref: rsid.cxx:58-75 — GF(16) multiply table: Squares[(a<<4)+b] == a*b in GF(16).
static const int Squares[] = {
	0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
	0, 1, 2, 3, 4, 5, 6, 7, 8, 9,10,11,12,13,14,15,
	0, 2, 4, 6, 8,10,12,14, 9,11,13,15, 1, 3, 5, 7,
	0, 3, 6, 5,12,15,10, 9, 1, 2, 7, 4,13,14,11, 8,
	0, 4, 8,12, 9,13, 1, 5,11,15, 3, 7, 2, 6,10,14,
	0, 5,10,15,13, 8, 7, 2, 3, 6, 9,12,14,11, 4, 1,
	0, 6,12,10, 1, 7,13,11, 2, 4,14, 8, 3, 5,15, 9,
	0, 7,14, 9, 5, 2,11,12,10,13, 4, 3,15, 8, 1, 6,
	0, 8, 9, 1,11, 3, 2,10,15, 7, 6,14, 4,12,13, 5,
	0, 9,11, 2,15, 6, 4,13, 7,14,12, 5, 8, 1, 3,10,
	0,10,13, 7, 3, 9,14, 4, 6,12,11, 1, 5,15, 8, 2,
	0,11,15, 4, 7,12, 8, 3,14, 5, 1,10, 9, 2, 6,13,
	0,12, 1,13, 2,14, 3,15, 4, 8, 5, 9, 6,10, 7,11,
	0,13, 3,14, 6,11, 5, 8,12, 1,15, 2,10, 7, 9, 4,
	0,14, 5,11,10, 4,15, 1,13, 3, 8, 6, 7, 9, 2,12,
	0,15, 7, 8,14, 1, 9, 6, 5,10, 2,13,11, 4,12, 3
};

// ref: rsid.cxx:77-79 — the 12 generator roots.
static const int indices[] = { 2, 4, 8, 9, 11, 15, 7, 14, 5, 10, 13, 3 };

// ref: rsid.cxx:184-196 — systematic RS(15,3) encode over GF(16). Verbatim.
static void Encode(int code, unsigned char *rsid) {
	rsid[0] = code >> 8;
	rsid[1] = (code >> 4) & 0x0f;
	rsid[2] = code & 0x0f;
	for (int i = 3; i < RSID_NSYMBOLS; i++)
		rsid[i] = 0;
	for (int i = 0; i < 12; i++) {
		for (int j = RSID_NSYMBOLS - 1; j > 0; j--)
			rsid[j] = rsid[j - 1] ^ Squares[(rsid[j] << 4) + indices[i]];
		rsid[0] = Squares[(rsid[0] << 4) + indices[i]];
	}
}

// ref: rsid_defs.cxx:29-217 — primary ID table (RSID_LIST), verbatim. The third
// macro arg is fldigi's internal trx_mode token; captured as a string only.
#define RSID_LIST                                       \
        ELEM_(263, EOT, MODE_EOT)                       \
        ELEM_(6, ESCAPE, NUM_MODES)                     \
        ELEM_(1, BPSK31, MODE_PSK31)                    \
        ELEM_(110, QPSK31, MODE_QPSK31)                 \
        ELEM_(2, BPSK63, MODE_PSK63)                    \
        ELEM_(3, QPSK63, MODE_QPSK63)                   \
        ELEM_(4, BPSK125, MODE_PSK125)                  \
        ELEM_(5, QPSK125, MODE_QPSK125)                 \
        ELEM_(126, BPSK250, MODE_PSK250)                \
        ELEM_(127, QPSK250, MODE_QPSK250)               \
        ELEM_(173, BPSK500, MODE_PSK500)                \
        ELEM_(183, PSK125R, MODE_PSK125R)               \
        ELEM_(186, PSK250R, MODE_PSK250R)               \
        ELEM_(187, PSK500R, MODE_PSK500R)               \
        ELEM_(7, PSKFEC31, NUM_MODES)                   \
        ELEM_(8, PSK10, NUM_MODES)                      \
        ELEM_(9, MT63_500_LG, MODE_MT63_500L)           \
        ELEM_(10, MT63_500_ST, MODE_MT63_500S)          \
        ELEM_(11, MT63_500_VST, NUM_MODES)              \
        ELEM_(12, MT63_1000_LG, MODE_MT63_1000L)        \
        ELEM_(13, MT63_1000_ST, MODE_MT63_1000S)        \
        ELEM_(14, MT63_1000_VST, NUM_MODES)             \
        ELEM_(15, MT63_2000_LG, MODE_MT63_2000L)        \
        ELEM_(17, MT63_2000_ST, MODE_MT63_2000S)        \
        ELEM_(18, MT63_2000_VST, NUM_MODES)             \
        ELEM_(19, PSKAM10, NUM_MODES)                   \
        ELEM_(20, PSKAM31, NUM_MODES)                   \
        ELEM_(21, PSKAM50, NUM_MODES)                   \
        ELEM_(22, PSK63F, MODE_PSK63F)                  \
        ELEM_(23, PSK220F, NUM_MODES)                   \
        ELEM_(24, CHIP64, NUM_MODES)                    \
        ELEM_(25, CHIP128, NUM_MODES)                   \
        ELEM_(26, CW, MODE_CW)                          \
        ELEM_(27, CCW_OOK_12, NUM_MODES)                \
        ELEM_(28, CCW_OOK_24, NUM_MODES)                \
        ELEM_(29, CCW_OOK_48, NUM_MODES)                \
        ELEM_(30, CCW_FSK_12, NUM_MODES)                \
        ELEM_(31, CCW_FSK_24, NUM_MODES)                \
        ELEM_(33, CCW_FSK_48, NUM_MODES)                \
        ELEM_(34, PACTOR1_FEC, NUM_MODES)               \
        ELEM_(113, PACKET_110, NUM_MODES)               \
        ELEM_(35, PACKET_300, NUM_MODES)                \
        ELEM_(36, PACKET_1200, NUM_MODES)               \
        ELEM_(37, RTTY_ASCII_7, MODE_RTTY)              \
        ELEM_(38, RTTY_ASCII_8, MODE_RTTY)              \
        ELEM_(39, RTTY_45, MODE_RTTY)                   \
        ELEM_(40, RTTY_50, MODE_RTTY)                   \
        ELEM_(41, RTTY_75, MODE_RTTY)                   \
        ELEM_(42, AMTOR_FEC, NUM_MODES)                 \
        ELEM_(43, THROB_1, MODE_THROB1)                 \
        ELEM_(44, THROB_2, MODE_THROB2)                 \
        ELEM_(45, THROB_4, MODE_THROB4)                 \
        ELEM_(46, THROBX_1, MODE_THROBX1)               \
        ELEM_(47, THROBX_2, MODE_THROBX2)               \
        ELEM_(146, THROBX_4, MODE_THROBX4)              \
        ELEM_(204, CONTESTIA_4_125, MODE_CONTESTIA_4_125)   \
        ELEM_(55,  CONTESTIA_4_250, MODE_CONTESTIA_4_250)   \
        ELEM_(54,  CONTESTIA_4_500, MODE_CONTESTIA_4_500)   \
        ELEM_(255, CONTESTIA_4_1000, MODE_CONTESTIA_4_1000) \
        ELEM_(254, CONTESTIA_4_2000, MODE_CONTESTIA_4_2000) \
        ELEM_(169, CONTESTIA_8_125, MODE_CONTESTIA_8_125)   \
        ELEM_(49,  CONTESTIA_8_250, MODE_CONTESTIA_8_250)   \
        ELEM_(52,  CONTESTIA_8_500, MODE_CONTESTIA_8_500)   \
        ELEM_(117, CONTESTIA_8_1000, MODE_CONTESTIA_8_1000) \
        ELEM_(247, CONTESTIA_8_2000, MODE_CONTESTIA_8_2000) \
        ELEM_(275, CONTESTIA_16_250, MODE_CONTESTIA_16_250)   \
        ELEM_(50,  CONTESTIA_16_500, MODE_CONTESTIA_16_500)   \
        ELEM_(53,  CONTESTIA_16_1000, MODE_CONTESTIA_16_1000) \
        ELEM_(259, CONTESTIA_16_2000, MODE_CONTESTIA_16_2000) \
        ELEM_(51,  CONTESTIA_32_1000, MODE_CONTESTIA_32_1000) \
        ELEM_(201, CONTESTIA_32_2000, MODE_CONTESTIA_32_2000) \
        ELEM_(194, CONTESTIA_64_500, MODE_CONTESTIA_64_500)   \
        ELEM_(193, CONTESTIA_64_1000, MODE_CONTESTIA_64_1000) \
        ELEM_(191, CONTESTIA_64_2000, MODE_CONTESTIA_64_2000) \
        ELEM_(56, VOICE, NUM_MODES)                     \
        ELEM_(60, MFSK8, MODE_MFSK8)                    \
        ELEM_(57, MFSK16, MODE_MFSK16)                  \
        ELEM_(147, MFSK32, MODE_MFSK32)                 \
        ELEM_(148, MFSK11, MODE_MFSK11)                 \
        ELEM_(152, MFSK22, MODE_MFSK22)                 \
        ELEM_(61, RTTYM_8_250, NUM_MODES)               \
        ELEM_(62, RTTYM_16_500, NUM_MODES)              \
        ELEM_(63, RTTYM_32_1000, NUM_MODES)             \
        ELEM_(65, RTTYM_8_500, NUM_MODES)               \
        ELEM_(66, RTTYM_16_1000, NUM_MODES)             \
        ELEM_(67, RTTYM_4_500, NUM_MODES)               \
        ELEM_(68, RTTYM_4_250, NUM_MODES)               \
        ELEM_(119, RTTYM_8_1000, NUM_MODES)             \
        ELEM_(170, RTTYM_8_125, NUM_MODES)              \
        ELEM_(203, OLIVIA_4_125, MODE_OLIVIA_4_125)     \
        ELEM_(75,  OLIVIA_4_250, MODE_OLIVIA_4_250)     \
        ELEM_(74,  OLIVIA_4_500, MODE_OLIVIA_4_500)     \
        ELEM_(229, OLIVIA_4_1000, MODE_OLIVIA_4_1000)   \
        ELEM_(238, OLIVIA_4_2000, MODE_OLIVIA_4_2000)   \
        ELEM_(163, OLIVIA_8_125, MODE_OLIVIA_8_125)     \
        ELEM_(69,  OLIVIA_8_250, MODE_OLIVIA_8_250)     \
        ELEM_(72,  OLIVIA_8_500, MODE_OLIVIA_8_500)     \
        ELEM_(116, OLIVIA_8_1000, MODE_OLIVIA_8_1000)   \
        ELEM_(214, OLIVIA_8_2000, MODE_OLIVIA_8_2000)   \
        ELEM_(70,  OLIVIA_16_500, MODE_OLIVIA_16_500)   \
        ELEM_(73,  OLIVIA_16_1000, MODE_OLIVIA_16_1000) \
        ELEM_(234, OLIVIA_16_2000, MODE_OLIVIA_16_2000) \
        ELEM_(71,  OLIVIA_32_1000, MODE_OLIVIA_32_1000) \
        ELEM_(221, OLIVIA_32_2000, MODE_OLIVIA_32_2000) \
        ELEM_(211, OLIVIA_64_2000, MODE_OLIVIA_64_2000) \
        ELEM_(76, PAX, NUM_MODES)                       \
        ELEM_(77, PAX2, NUM_MODES)                      \
        ELEM_(78, DOMINOF, NUM_MODES)                   \
        ELEM_(79, FAX, NUM_MODES)                       \
        ELEM_(81, SSTV, NUM_MODES)                      \
        ELEM_(84, DOMINOEX_4, MODE_DOMINOEX4)           \
        ELEM_(85, DOMINOEX_5, MODE_DOMINOEX5)           \
        ELEM_(86, DOMINOEX_8, MODE_DOMINOEX8)           \
        ELEM_(87, DOMINOEX_11, MODE_DOMINOEX11)         \
        ELEM_(88, DOMINOEX_16, MODE_DOMINOEX16)         \
        ELEM_(90, DOMINOEX_22, MODE_DOMINOEX22)         \
        ELEM_(92, DOMINOEX_4_FEC, MODE_DOMINOEX4)       \
        ELEM_(93, DOMINOEX_5_FEC, MODE_DOMINOEX5)       \
        ELEM_(97, DOMINOEX_8_FEC, MODE_DOMINOEX8)       \
        ELEM_(98, DOMINOEX_11_FEC, MODE_DOMINOEX11)     \
        ELEM_(99, DOMINOEX_16_FEC, MODE_DOMINOEX16)     \
        ELEM_(101, DOMINOEX_22_FEC, MODE_DOMINOEX22)    \
        ELEM_(104, FELD_HELL, MODE_FELDHELL)            \
        ELEM_(105, PSK_HELL, NUM_MODES)                 \
        ELEM_(106, HELL_80, MODE_HELL80)                \
        ELEM_(107, FM_HELL_105, MODE_FSKH105)           \
        ELEM_(108, FM_HELL_245, MODE_FSKH245)           \
        ELEM_(114, MODE_141A, NUM_MODES)                \
        ELEM_(123, DTMF, NUM_MODES)                     \
        ELEM_(125, ALE400, NUM_MODES)                   \
        ELEM_(131, FDMDV, NUM_MODES)                    \
        ELEM_(132, JT65_A, NUM_MODES)                   \
        ELEM_(134, JT65_B, NUM_MODES)                   \
        ELEM_(135, JT65_C, NUM_MODES)                   \
        ELEM_(136, THOR_4, MODE_THOR4)                  \
        ELEM_(137, THOR_8, MODE_THOR8)                  \
        ELEM_(138, THOR_16, MODE_THOR16)                \
        ELEM_(139, THOR_5, MODE_THOR5)                  \
        ELEM_(143, THOR_11, MODE_THOR11)                \
        ELEM_(145, THOR_22, MODE_THOR22)                \
        ELEM_(153, CALL_ID, NUM_MODES)                  \
        ELEM_(155, PACKET_PSK1200, NUM_MODES)           \
        ELEM_(156, PACKET_PSK250, NUM_MODES)            \
        ELEM_(159, PACKET_PSK63, NUM_MODES)             \
        ELEM_(172, MODE_188_110A_8N1, NUM_MODES)        \
        ELEM_(0, NONE, NUM_MODES)

// ref: rsid_defs.cxx:232-306 — extended ID table (RSID_LIST2), verbatim.
#define RSID_LIST2                                      \
        ELEM2_(450, PSK63RX4, MODE_4X_PSK63R)           \
        ELEM2_(457, PSK63RX5, MODE_5X_PSK63R)           \
        ELEM2_(458, PSK63RX10, MODE_10X_PSK63R)         \
        ELEM2_(460, PSK63RX20, MODE_20X_PSK63R)         \
        ELEM2_(462, PSK63RX32, MODE_32X_PSK63R)         \
        ELEM2_(467, PSK125RX4, MODE_4X_PSK125R)         \
        ELEM2_(497, PSK125RX5, MODE_5X_PSK125R)         \
        ELEM2_(513, PSK125RX10, MODE_10X_PSK125R)       \
        ELEM2_(519, PSK125X12, MODE_12X_PSK125)         \
        ELEM2_(522, PSK125RX12, MODE_12X_PSK125R)       \
        ELEM2_(527, PSK125RX16, MODE_16X_PSK125R)       \
        ELEM2_(529, PSK250RX2, MODE_2X_PSK250R)         \
        ELEM2_(533, PSK250RX3, MODE_3X_PSK250R)         \
        ELEM2_(539, PSK250RX5, MODE_5X_PSK250R)         \
        ELEM2_(541, PSK250X6, MODE_6X_PSK250)           \
        ELEM2_(545, PSK250RX6, MODE_6X_PSK250R)         \
        ELEM2_(551, PSK250RX7, MODE_7X_PSK250R)         \
        ELEM2_(553, PSK500RX2, MODE_2X_PSK500R)         \
        ELEM2_(558, PSK500RX3, MODE_3X_PSK500R)         \
        ELEM2_(564, PSK500RX4, MODE_4X_PSK500R)         \
        ELEM2_(566, PSK500X2, MODE_2X_PSK500)           \
        ELEM2_(569, PSK500X4, MODE_4X_PSK500)           \
        ELEM2_(570, PSK1000, MODE_PSK1000)              \
        ELEM2_(580, PSK1000R, MODE_PSK1000R)            \
        ELEM2_(587, PSK1000X2, MODE_2X_PSK1000)         \
        ELEM2_(595, PSK1000RX2, MODE_2X_PSK1000R)       \
        ELEM2_(604, PSK800RX2, MODE_2X_PSK800R)         \
        ELEM2_(610, PSK800X2, MODE_2X_PSK800)           \
        ELEM2_(620, MFSK64, MODE_MFSK64)                \
        ELEM2_(625, MFSK128, MODE_MFSK128)              \
        ELEM2_(639, THOR25x4, MODE_THOR25x4)            \
        ELEM2_(649, THOR50x1, MODE_THOR50x1)            \
        ELEM2_(653, THOR50x2, MODE_THOR50x2)            \
        ELEM2_(658, THOR100, MODE_THOR100)              \
        ELEM2_(662, DOMINOEX_44, MODE_DOMINOEX44)       \
        ELEM2_(681, DOMINOEX_88, MODE_DOMINOEX88)       \
        ELEM2_(687, MFSK31, MODE_MFSK31)                \
        ELEM2_(691, DOMINOEX_MICRO, MODE_DOMINOEXMICRO) \
        ELEM2_(693, THOR_MICRO, MODE_THORMICRO)         \
        ELEM2_(1026, MFSK64L, MODE_MFSK64L)             \
        ELEM2_(1029, MFSK128L, MODE_MFSK128L)           \
        ELEM2_(1066, PSK8P125, MODE_8PSK125)            \
        ELEM2_(1071, PSK8P250, MODE_8PSK250)            \
        ELEM2_(1076, PSK8P500, MODE_8PSK500)            \
        ELEM2_(1047, PSK8P1000, MODE_8PSK1000)          \
        ELEM2_(1037, PSK8P125F, MODE_8PSK125F)          \
        ELEM2_(1038, PSK8P250F, MODE_8PSK250F)          \
        ELEM2_(1043, PSK8P500F, MODE_8PSK500F)          \
        ELEM2_(1078, PSK8P1000F, MODE_8PSK1000F)        \
        ELEM2_(1058, PSK8P1200F, MODE_8PSK1200F)        \
        ELEM2_(1239, PSK8P125FL, MODE_8PSK125FL)        \
        ELEM2_(2052, PSK8P250FL, MODE_8PSK250FL)        \
        ELEM2_(2053, OFDM500F, MODE_OFDM_500F)          \
        ELEM2_(2094, OFDM7F0F, MODE_OFDM_750F)          \
        ELEM2_(2118, OFDM2000, MODE_OFDM_2000)          \
        ELEM2_(2110, OFDM2000F, MODE_OFDM_2000F)        \
        ELEM2_(1171, IFKP, MODE_IFKP)                   \
        ELEM2_(0, NONE2, NUM_MODES)

int main() {
	printf("{\"_meta\":{"
	       "\"upstream\":\"w1hkj/fldigi @ ../fldigi\","
	       "\"files\":["
	       "\"src/rsid/rsid.cxx:58-79 (Squares,indices verbatim)\","
	       "\"src/rsid/rsid.cxx:184-196 (Encode verbatim)\","
	       "\"src/rsid/rsid_defs.cxx:29-217 (RSID_LIST)\","
	       "\"src/rsid/rsid_defs.cxx:232-306 (RSID_LIST2)\"],"
	       "\"driver\":\"scratch/refvectors/build_rsid.sh\","
	       "\"note\":\"syms are the 15 RS(15,3)/GF(16) symbols (tone indices 0..15) Encode(code) emits; bit-exact.\""
	       "}}\n");

	unsigned char rsid[RSID_NSYMBOLS];

#define EMIT(tbl, code_, tag_, mode_)                                    \
	Encode((code_), rsid);                                               \
	printf("{\"table\":%d,\"code\":%d,\"tag\":\"%s\",\"mode\":\"%s\",\"syms\":[", \
	       (tbl), (code_), #tag_, #mode_);                               \
	for (int i = 0; i < RSID_NSYMBOLS; i++)                              \
		printf("%s%d", i ? "," : "", rsid[i]);                           \
	printf("]}\n");

#define ELEM_(code_, tag_, mode_)  EMIT(1, code_, tag_, mode_)
	RSID_LIST
#undef ELEM_
#define ELEM2_(code_, tag_, mode_) EMIT(2, code_, tag_, mode_)
	RSID_LIST2
#undef ELEM2_

	return 0;
}
