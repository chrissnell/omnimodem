//! FST4/FST4W LDPC codes, ported from WSJT-X `wsjtx/lib/fst4/`.
//!
//! The (240,101) code carries a 77-bit message + 24-bit CRC; the systematic
//! generator below is transcribed verbatim from the reference generator table
//! and the encode replicates `encode240_101.f90` exactly (bit-exact vs the
//! reference `fst4_ldpc_dump` golden vector). ref: wsjtx/lib/fst4/
//! encode240_101.f90 + ldpc_240_101_generator.f90 (upstream WSJTX/wsjtx).

/// Generator rows: 139 parity checks, each 26 hex chars (104 bits, first 101
/// used). ref: wsjtx/lib/fst4/ldpc_240_101_generator.f90 (`character*26 g(139)`).
const G_240_101: [&str; 139] = [
    "e28df133efbc554bcd30eb1828",
    "b1adf97787f81b4ac02e0caff8",
    "e70c43adce5036f847af367560",
    "c26663f7f7acafdf5abacb6f30",
    "eba93204ddfa3bcf994aea8998",
    "126b51e33c6a740afa0d5ce990",
    "b41a1569e6fede1f2f5395cb68",
    "1d3af0bb43fddbc670a291cc70",
    "e0aebd9921e2c9e1d453ffccb0",
    "897d1370f0df94b8b27a5e4fb8",
    "5e97539338003b13fa8198ad38",
    "7276b87da4a4d777e2752fdd48",
    "989888bd3a85835e2bc6a560f8",
    "7ec4f4a56199ab0a8d6e102478",
    "207007665090258782d1b38a98",
    "1ea1f61cd7f0b7eed7dd346ab8",
    "08f150b27c7f18a027783de0e8",
    "d42324a4e21b62d548d7865858",
    "2e029656269d4fe46e167d21d0",
    "7d84acb7737b0ca6b6f2ef5eb0",
    "6674ca04528ad4782bf5e15248",
    "118ce9825f563ae4963af7a0b0",
    "fb06248cc985e314b1b36ccd38",
    "1c478b7a5aec7e1cfc9c24eb70",
    "185a0f06a84f7f4f484c455020",
    "98b840a3a70688cd58588e3e30",
    "cfb7719de83a3baf582e5b2aa0",
    "9d8cc6b5a01fdbfa307a769048",
    "ed776a728ca162d6fcc8996760",
    "8d2b068128dfb2f8d22c79db50",
    "bd2ba50007789ffb7324aa9190",
    "fd95008fe88812025e78065610",
    "3027849be8e99f9ef68eac1020",
    "88574e1ea39d87414b15e803a8",
    "89365b330e76e6dde740dced08",
    "c83f37b913ed0f6b802aaf21d8",
    "bdca7c1959caa7488b7eb13030",
    "794e0b4888e1ef42992287dd98",
    "526ac87fbaa790c6cd58864e08",
    "940518ba1a51c1da55bc8b2d70",
    "59c5e51ebfbd02ab30ff822378",
    "c81fff87866e04f8f3948c7f10",
    "7913513f3e2a3c0f76b69f6d68",
    "e43cc04da189c44803c4f740a0",
    "fdca7c1959ca85488b7eb13030",
    "95b07fce9b7b1bf4f057ca61b8",
    "d7db48a86691a0c0c9305aac90",
    "0d50bf79a59464597c43ba8058",
    "4a9c34b23fd5eaff8c9dc215e0",
    "3d5305a6f0427938eeb9d1c118",
    "55d8b6b58039f7a3a2d592a900",
    "784f349ecb74c4abbdbb073b90",
    "5973bbb2205f9d6a5c9a55c238",
    "5d2ee61006fec94f69f6b0f460",
    "9e1f52ef1e6589990dd0ce0cc8",
    "85b7b48f4b45775c9f8a36cc90",
    "ae1d6a0171168f6d70804b79f8",
    "a467aa9aa6cdc7094677c730d8",
    "dcf2f56c9ae20fb57e89b916d0",
    "3ae98d26ae96ea714c1a5146d0",
    "103c89581446805b8c71b2e638",
    "6783f3dfec835dd4e92131cc20",
    "52f88428c50f12c55876f7d8a8",
    "51fcb0e56a22fa3b7140aeaa80",
    "07c54871155603e65325f66cd8",
    "a8dd4fac47a113ee5706eef180",
    "f6cdc6f4cc1fa7e4db15bf86f8",
    "2e1c6a0171168f6d70c04a79f8",
    "2a90ab82bef6424db981752dc8",
    "845a1db59c193249d937e889d0",
    "a929d379f1769cb4baa4e41e90",
    "0c2a5829548d82223d6f566d48",
    "420087bc5c4e2f5bc139ad0220",
    "6df8d880ae7209fe52c69ede00",
    "dfbdcef29a985fd40d052d1a88",
    "8567fc332342b1ed8408f5fa00",
    "c908feb4e1866a24ca0c702a08",
    "645f5ee59f9f64fd43a5f2ec30",
    "bee56991e877baf3e9cf11b770",
    "649ea2e4194ca51be28abf3430",
    "90e7394c551bd58d00686d5420",
    "4e3cf731f8f89e8414214afaf0",
    "dcbf16aa8180a7712571e94f98",
    "9b456c015999c52b7fbd1ab390",
    "397ab76924659c4b8b3be4ac58",
    "4f5038c4f9da4b02bdfa178278",
    "4892fada978c98dd4fd363c450",
    "6c8af64b426bc474431c110c98",
    "84a553be5ef0e57390a5af05b0",
    "bed4a9347c9a2064f6d63ac0f8",
    "d973bbb2605f9d6a5c9a57c238",
    "1e3bee9a99fe10d3864ee669d8",
    "a590771ff185d807cb32f46000",
    "9a498fc4b549d81c625f80fc90",
    "28b3e72878aadee7e0e2617950",
    "96ce025d621a91396aa8f3ec20",
    "4f5a77becf838a590d6d406ea8",
    "52d3856dfb9fe78012f10e25c0",
    "b45323c2b28b4752ca0675d2e0",
    "3bae5a8452a785beb35851ad18",
    "65098832d20d915e75bea336e8",
    "5eb6f3c331098e8c0fbfa3aee0",
    "ef19d974a25540c8998fbf1df0",
    "403ea58feff08cf92d5cacc780",
    "6ba93204ddfa7bcb994aea8998",
    "653909166aa7bead4bd9c90020",
    "089cb20e639bc5a44da66f17c0",
    "10f803949961359e994f5ade88",
    "15b7ec1e6106cd55ef7d996590",
    "c99e99de9d85d2b999a17a95d8",
    "ca3e161b97148bac6dd28a6178",
    "e1ab199c992cb4c22aee115358",
    "ea8a4d0e96d3d9f827899b6d88",
    "8af4992d60223f021569a8ab60",
    "5087771abceb87a6d872291fe8",
    "d045e0812e217bb7bbdac92f30",
    "ccccd78ae5fa6e191f21c06908",
    "54545f37df6fed4734ef6509b0",
    "b0780327d899cbc03d95a81a48",
    "a4229c31f2b85e44a322273d50",
    "d182ab001c2085ea7be26a20d0",
    "1a82c30b4fba7dfaafb8d287a8",
    "d974fba598e7fb0630c1587db0",
    "b5c078a8cbab3e73728659ea20",
    "626bbf9eed1a8715c3a7d38f60",
    "c1efe9aa67130865fda93d8be8",
    "d39796dbce155df6306e7b77c0",
    "c7e7c1f032d7209b4549e84aa8",
    "d5799b30a1605baf6b9cd04960",
    "0baf2d21051a926dfd87046d70",
    "da8bf7d1e305c499b573c02cc8",
    "0ccaa7fffb9ae3e42dd0688328",
    "b951b62e18f5290ac13c195130",
    "79b006f001961fb233be80d0e8",
    "56637b6dedfd6e050f06404a48",
    "e0c4bf71a15597523bbd57bde0",
    "1312231ffa04426a34a8fab038",
    "db5f6f0455d24b8358d1cbc3d8",
    "d559e31b34d21f48e1f501af30",
];

/// Build the (240,101) generator matrix from the hex table, exactly as
/// `encode240_101.f90`: each hex char contributes 4 bits MSB-first, except the
/// 26th char contributes only 1 bit (101 columns total). ref: encode240_101.f90.
#[allow(clippy::needless_range_loop)] // hex-nibble index mirrors the Fortran fill
fn gen_240_101() -> [[u8; 101]; 139] {
    let mut gen = [[0u8; 101]; 139];
    for (i, row) in G_240_101.iter().enumerate() {
        let b = row.as_bytes();
        for j in 0..26 {
            let istr = (b[j] as char).to_digit(16).unwrap() as u8;
            let ibmax = if j == 25 { 1 } else { 4 };
            for jj in 1..=ibmax {
                let icol = j * 4 + (jj - 1);
                if (istr >> (4 - jj)) & 1 == 1 {
                    gen[i][icol] = 1;
                }
            }
        }
    }
    gen
}

/// Systematic encode of a 101-bit message (77-bit payload + 24-bit CRC) into the
/// 240-bit FST4 codeword: `codeword = message ++ parity`, `parity[i] =
/// sum_j message[j]*gen[i][j] mod 2`. ref: encode240_101.f90.
pub fn encode_240_101(message: &[u8; 101]) -> [u8; 240] {
    let gen = gen_240_101();
    let mut cw = [0u8; 240];
    cw[..101].copy_from_slice(message);
    for i in 0..139 {
        let mut nsum = 0u32;
        for j in 0..101 {
            nsum += u32::from(message[j] & gen[i][j]);
        }
        cw[101 + i] = (nsum % 2) as u8;
    }
    cw
}


/// (240,74) generator rows: 166 parity checks, each 19 hex chars (76 bits,
/// first 74 used). ref: wsjtx/lib/fst4/ldpc_240_74_generator.f90.
const G_240_74: [&str; 166] = [
    "de8b3201e3c59f55a14",
    "2e06d352ebc5b74c4fc",
    "2e16d6cf5a725c3244c",
    "84f5587edca6d777de4",
    "e152b1e2b5965093ecc",
    "244b4828a2ccf2b5f58",
    "5fbbaade810e123c730",
    "6b7e92a99a918df3d44",
    "bbcec6a63ab757a7278",
    "f5f3f0b89a21ceccdb0",
    "a248c5f1ec2bc816290",
    "c84bbad839a5fe76d0c",
    "ad724129bbf4c7f4570",
    "91adb56e7623a2575cc",
    "cbe995bdf156df2c9e4",
    "92ff6ea492c08c150e0",
    "c4ddbe5a02f6a933384",
    "d2e9befc131dc483858",
    "68567543d1eebcb080c",
    "21fa61d559f9baf6abc",
    "911c4fbbafc72e3db28",
    "7c0b534af4b7d583d50",
    "12ce371b90ee9dfe72c",
    "15a604148872e251ec4",
    "3a3c9f3eb0e0f96edc0",
    "705919ffb636f96b390",
    "43daaaa8163d6bc2bd4",
    "96e11ea798b74b10e98",
    "811150609c9dee8230c",
    "be713f85ab34380f4b0",
    "5a02c4abaaccb8f24c4",
    "67bdebb8863d04768cc",
    "5a449cd90c3dbdfe844",
    "9c7a54d1c4ef7418b84",
    "cd82fefaaf9cd28cd8c",
    "ca47e847fabb0054a38",
    "f0b30cef6aab9e37f98",
    "d948d912fbcc1708710",
    "cce1a7b355053d98270",
    "4cf227c225a9063dd48",
    "2db92612e9ba1418e24",
    "3d215c04c762c3d6a28",
    "77de65500b5624ceb0c",
    "fd1a1df99ded2fb9d88",
    "2a19392c71438410fb8",
    "a9b486a9d26ed579754",
    "b698d244ac78d97a498",
    "3d7975b74d727a5e704",
    "38094225a2bce0e1940",
    "3d3e58fae40fac342b0",
    "7732e839a066e337714",
    "69356c082b7753a47b0",
    "3e868a55dc403a802ac",
    "a0157a14a6bf7fdbbcc",
    "1ab628e11a7ab4a7c44",
    "9da3a2247d7449052f4",
    "199a8a7b114816b97f4",
    "b1c5cde2542061704cc",
    "432fa8d3a153eafbdc8",
    "c4ece7e400d8a89c448",
    "316ecf74e4b983f007c",
    "6a14fa8e713bb5e8adc",
    "da4b957ded8374e3640",
    "0a804dba7c7e4533300",
    "52c342ed033f86580e0",
    "1667da8d6fcf4272470",
    "da2f7038d550fa88d8c",
    "685bcbab1d9dd2c2a44",
    "4c93008b3156b3636bc",
    "726998d6327ac797c3c",
    "44ece7e400d8a8dc448",
    "01f9add00dfe823a948",
    "dbb95f5ce9e371ad720",
    "fc746ee5c76827a8728",
    "b25408029506467f4b4",
    "9b5c9219e21126b7cf8",
    "39ae9f48ba9d1a24f04",
    "7de2699623eb507f938",
    "b9c6e903ee91dd32934",
    "397510d2c6cb5e81de8",
    "20157a14aebf7fdbbec",
    "067f76ea5817a465980",
    "9248f3cea0869feb994",
    "23cde2678004ebe5f80",
    "5b81fe6848f58e3cfa8",
    "a9099ace96bff092904",
    "4afa4b0802b33215438",
    "f4f740396b030360858",
    "fc613f77a35ee1163b8",
    "1a4dc27d7e8cc835ff4",
    "e9b056f153b39def7ec",
    "b62eb777a2f953c7efc",
    "388ae4de514b62d238c",
    "891529af40e85317160",
    "474f1afeb724dbd2ba8",
    "11d70880fd88fdd307c",
    "29f26a3acb76e6a517c",
    "df3e902ff9cadcf776c",
    "e3c42da8445965c09f0",
    "ce277a6aeccc316dc58",
    "4d7841fb71543abd9b8",
    "e63230d2d465fb44750",
    "b6e11fa798b74b14e98",
    "05f189d37c5616547b4",
    "ebdb51a81d1e883baa8",
    "bf5bc736663bcd53ae0",
    "2f8d1cc0936142c08fc",
    "436b22fc36d917b6928",
    "044b482822ccf2b5f58",
    "37b2e839a066e3b7714",
    "2a9b4b765c581f0c51c",
    "10a7d44cecf8e6628dc",
    "ad95f02df6d5502dd4c",
    "bbd34f8afd63deaf564",
    "cabddfeb01fce632788",
    "66b57babeedd6124114",
    "7813e0454fbd462be8c",
    "b6105ed6f01ea621d04",
    "9f68bbcec679d1c088c",
    "673da96e414fc7a0f40",
    "5568adb935e11084abc",
    "f6dd308de5e5c4f6fb0",
    "3b49e80d40ae596c7b4",
    "a3cde2478004ebe5f80",
    "dd8e4f309e919d5ed94",
    "5a4020d387757d7bc28",
    "64f9e02ae32362a255c",
    "630d5942d392334b0dc",
    "0bd7e9f4229b2dee210",
    "bca549a9467d3a2550c",
    "2fef7b1f578c5e28d04",
    "f35e0fdda1be4b3b35c",
    "69ed575e7cc537d2394",
    "7dfdcfbfd5ef3093680",
    "b3b2921af97f251d328",
    "5622d0fe90363522364",
    "fcd4fc7fa04a69d2ac4",
    "1119ea451502ed9ab34",
    "970ee777ec969a41754",
    "688d14f8afec76783dc",
    "4d0b8a1028578407420",
    "d3d2138d9fa268da3e8",
    "df1bdbff898e006394c",
    "8ac478a916bb0b77684",
    "93881997428e2c17a94",
    "4aa510e746245e90c08",
    "e00cb8543f85a5d58b8",
    "9100d8eb74031073044",
    "38710e4235bd1e4003c",
    "6aef311cac4c4dccfd4",
    "58430f577f51c36b3e0",
    "12082fa5d4268a95b4c",
    "7a7435a0aca071e64d0",
    "cd8250ebadc95de15b0",
    "debad40c852e99d64dc",
    "4e6caa5e7c86efef748",
    "a5d4cbb97e726e3c580",
    "7e3a0a2c73ef8553640",
    "b60bfc2fd2bd8f530dc",
    "32dbef097a5f84b0318",
    "4cc7c1cf434300be380",
    "896840945be8eabf7f0",
    "36c9b10ec694819a0a0",
    "349f46a799ef95a47c8",
    "9bdcd4ce2563e560b74",
    "b19fcd7111a335c52ec",
];

/// Build the (240,74) generator matrix from the hex table, exactly as
/// `encode240_74.f90`: 4 bits MSB-first per hex char, except the 19th char
/// contributes 2 bits (74 columns total). ref: encode240_74.f90.
#[allow(clippy::needless_range_loop)] // hex-nibble index mirrors the Fortran fill
fn gen_240_74() -> [[u8; 74]; 166] {
    let mut gen = [[0u8; 74]; 166];
    for (i, row) in G_240_74.iter().enumerate() {
        let b = row.as_bytes();
        for j in 0..19 {
            let istr = (b[j] as char).to_digit(16).unwrap() as u8;
            let ibmax = if j == 18 { 2 } else { 4 };
            for jj in 1..=ibmax {
                let icol = j * 4 + (jj - 1);
                if (istr >> (4 - jj)) & 1 == 1 {
                    gen[i][icol] = 1;
                }
            }
        }
    }
    gen
}

/// Systematic encode of a 74-bit message (50-bit WSPR-format beacon + 24-bit
/// CRC) into the 240-bit FST4W codeword. ref: encode240_74.f90.
pub fn encode_240_74(message: &[u8; 74]) -> [u8; 240] {
    let gen = gen_240_74();
    let mut cw = [0u8; 240];
    cw[..74].copy_from_slice(message);
    for i in 0..166 {
        let mut nsum = 0u32;
        for j in 0..74 {
            nsum += u32::from(message[j] & gen[i][j]);
        }
        cw[74 + i] = (nsum % 2) as u8;
    }
    cw
}


use crate::fec::ldpc::Ldpc;

/// Sparse parity-check Tanner graph: 139 checks, each up to 6 one-origin
/// codeword-variable indices (0 = padding). ref: wsjtx/lib/fst4/
/// ldpc_240_101_parity.f90 (`data Nm`).
const NM_240_101: [[u16; 6]; 139] = [
    [3, 52, 95, 102, 140, 182],
    [4, 53, 96, 103, 108, 210],
    [5, 54, 97, 104, 150, 194],
    [6, 55, 98, 105, 136, 187],
    [7, 56, 99, 106, 139, 182],
    [8, 57, 100, 107, 149, 193],
    [9, 58, 101, 103, 135, 186],
    [10, 59, 109, 172, 200, 0],
    [11, 60, 110, 157, 207, 0],
    [12, 61, 111, 122, 204, 0],
    [13, 62, 112, 117, 205, 0],
    [3, 39, 113, 137, 195, 0],
    [14, 63, 114, 133, 202, 0],
    [15, 64, 115, 116, 203, 0],
    [3, 60, 115, 183, 209, 0],
    [16, 65, 112, 124, 194, 0],
    [11, 66, 118, 156, 197, 0],
    [17, 67, 119, 184, 208, 0],
    [18, 36, 101, 120, 145, 212],
    [19, 37, 108, 121, 211, 0],
    [20, 68, 122, 153, 193, 0],
    [17, 61, 123, 124, 214, 0],
    [4, 51, 123, 152, 219, 0],
    [21, 68, 125, 139, 202, 0],
    [22, 69, 100, 126, 138, 194],
    [21, 70, 127, 144, 213, 0],
    [23, 71, 128, 169, 213, 0],
    [24, 72, 129, 166, 195, 0],
    [25, 73, 99, 130, 163, 215],
    [10, 28, 131, 155, 217, 0],
    [26, 74, 100, 132, 179, 224],
    [27, 75, 114, 143, 216, 0],
    [28, 76, 134, 141, 221, 0],
    [12, 77, 135, 142, 222, 0],
    [29, 76, 97, 136, 178, 205],
    [13, 78, 137, 153, 220, 0],
    [20, 79, 126, 143, 217, 0],
    [8, 80, 125, 166, 223, 0],
    [69, 102, 151, 218, 238, 0],
    [9, 52, 134, 177, 225, 0],
    [30, 66, 96, 142, 201, 226],
    [30, 81, 126, 176, 229, 0],
    [82, 127, 164, 227, 231, 0],
    [31, 60, 120, 133, 228, 0],
    [32, 33, 146, 196, 230, 0],
    [33, 46, 147, 156, 231, 0],
    [34, 77, 148, 162, 229, 0],
    [35, 83, 148, 149, 232, 0],
    [23, 62, 104, 175, 215, 0],
    [15, 84, 148, 151, 214, 0],
    [36, 51, 107, 117, 233, 0],
    [37, 85, 127, 137, 216, 0],
    [38, 52, 154, 166, 203, 0],
    [39, 65, 155, 171, 224, 0],
    [8, 63, 147, 155, 211, 0],
    [2, 74, 110, 168, 236, 0],
    [1, 16, 74, 88, 158, 169],
    [40, 86, 159, 180, 223, 0],
    [87, 160, 177, 234, 237, 0],
    [18, 56, 124, 161, 227, 0],
    [20, 47, 123, 167, 234, 0],
    [41, 86, 130, 170, 229, 0],
    [42, 54, 111, 188, 227, 0],
    [35, 72, 165, 180, 204, 0],
    [29, 88, 129, 145, 235, 0],
    [40, 50, 161, 189, 236, 0],
    [37, 89, 110, 170, 212, 0],
    [7, 55, 128, 172, 181, 0],
    [67, 70, 117, 130, 237, 238],
    [42, 83, 96, 116, 147, 225],
    [42, 69, 128, 168, 186, 0],
    [4, 57, 144, 173, 207, 0],
    [43, 53, 94, 164, 174, 209],
    [24, 79, 150, 174, 208, 0],
    [21, 79, 145, 165, 225, 0],
    [44, 64, 160, 189, 220, 0],
    [45, 82, 136, 154, 217, 0],
    [9, 56, 132, 133, 205, 0],
    [46, 80, 120, 150, 181, 0],
    [41, 63, 109, 140, 187, 0],
    [47, 53, 105, 188, 215, 0],
    [48, 81, 183, 185, 230, 0],
    [16, 90, 95, 119, 190, 216],
    [10, 87, 185, 188, 228, 0],
    [44, 91, 158, 163, 211, 0],
    [49, 78, 173, 176, 228, 0],
    [30, 92, 158, 180, 209, 0],
    [22, 40, 178, 184, 210, 0],
    [18, 59, 95, 118, 192, 222],
    [14, 70, 191, 206, 230, 0],
    [26, 75, 140, 142, 231, 0],
    [48, 83, 131, 157, 208, 0],
    [6, 61, 160, 183, 197, 0],
    [32, 73, 149, 174, 187, 0],
    [12, 64, 178, 196, 213, 0],
    [27, 93, 101, 105, 141, 236],
    [33, 89, 141, 198, 239, 0],
    [19, 93, 138, 199, 204, 0],
    [2, 82, 99, 116, 119, 200],
    [1, 50, 62, 106, 206, 0],
    [32, 71, 112, 201, 207, 0],
    [34, 75, 111, 157, 233, 0],
    [39, 73, 186, 199, 206, 0],
    [27, 35, 38, 171, 181, 237],
    [49, 93, 118, 175, 202, 0],
    [45, 80, 121, 167, 240, 0],
    [19, 94, 161, 179, 203, 0],
    [31, 86, 106, 164, 234, 0],
    [45, 84, 104, 177, 226, 0],
    [47, 66, 102, 196, 212, 0],
    [25, 65, 98, 173, 192, 240],
    [49, 85, 179, 191, 232, 0],
    [38, 89, 153, 175, 210, 0],
    [25, 43, 125, 168, 214, 0],
    [22, 58, 113, 172, 239, 0],
    [36, 68, 184, 191, 226, 0],
    [15, 78, 103, 143, 235, 0],
    [50, 84, 152, 171, 222, 0],
    [24, 85, 135, 185, 238, 0],
    [48, 59, 134, 169, 193, 0],
    [17, 91, 176, 182, 198, 0],
    [11, 88, 113, 167, 232, 0],
    [43, 81, 122, 132, 239, 0],
    [41, 58, 107, 190, 197, 0],
    [6, 91, 144, 159, 224, 0],
    [46, 72, 131, 151, 220, 0],
    [23, 90, 152, 198, 223, 0],
    [44, 77, 98, 114, 218, 219],
    [26, 76, 165, 190, 240, 0],
    [28, 54, 115, 170, 219, 0],
    [31, 57, 97, 146, 163, 195],
    [14, 92, 108, 162, 221, 0],
    [5, 71, 121, 139, 233, 0],
    [1, 34, 87, 154, 192, 0],
    [29, 90, 156, 199, 218, 0],
    [2, 51, 55, 138, 146, 0],
    [5, 92, 109, 189, 235, 0],
    [13, 94, 159, 162, 200, 0],
    [7, 67, 129, 201, 221, 0],
];

/// Row weights (valid entries per check). ref: same file (`data nrw`).
const NRW_240_101: [u8; 139] = [6, 6, 6, 6, 6, 6, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 6, 5, 5, 5, 6, 5, 6, 5, 5, 5, 6, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 5, 5, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 5, 6, 5, 5, 6, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 5, 6, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 5, 5, 6, 5, 5, 5, 5, 5, 5, 5, 5];

/// (240,74) sparse Tanner graph: 166 checks x up to 5 one-origin vars
/// (0 = pad). ref: wsjtx/lib/fst4/ldpc_240_74_parity.f90 (`data Nm`).
const NM_240_74: [[u16; 5]; 166] = [
    [4, 62, 75, 121, 191],
    [5, 63, 76, 82, 175],
    [6, 64, 77, 112, 206],
    [7, 65, 78, 151, 180],
    [8, 66, 79, 159, 188],
    [9, 67, 80, 127, 164],
    [68, 81, 119, 182, 237],
    [10, 69, 76, 109, 173],
    [70, 83, 141, 181, 230],
    [2, 71, 84, 185, 193],
    [11, 64, 85, 91, 190],
    [12, 72, 86, 127, 196],
    [13, 73, 87, 110, 194],
    [2, 74, 88, 106, 192],
    [14, 88, 89, 200, 0],
    [15, 90, 120, 197, 0],
    [16, 85, 131, 188, 0],
    [12, 65, 92, 165, 197],
    [17, 93, 124, 195, 0],
    [18, 94, 148, 199, 0],
    [19, 95, 115, 204, 0],
    [10, 96, 98, 207, 0],
    [20, 97, 116, 205, 0],
    [13, 44, 96, 129, 190],
    [21, 99, 113, 196, 0],
    [22, 100, 116, 175, 0],
    [23, 101, 139, 212, 0],
    [11, 102, 147, 175, 0],
    [24, 74, 103, 139, 216],
    [25, 72, 104, 118, 182],
    [26, 105, 165, 209, 0],
    [27, 106, 111, 213, 0],
    [18, 107, 122, 194, 0],
    [28, 56, 108, 162, 214],
    [59, 82, 107, 208, 235],
    [23, 87, 144, 217, 0],
    [17, 73, 111, 172, 181],
    [29, 112, 132, 214, 0],
    [30, 42, 99, 185, 186],
    [31, 71, 114, 146, 215],
    [16, 95, 143, 199, 0],
    [32, 97, 145, 210, 0],
    [33, 117, 118, 218, 0],
    [15, 117, 122, 220, 0],
    [34, 100, 119, 200, 0],
    [35, 99, 120, 219, 0],
    [16, 75, 127, 221, 0],
    [36, 117, 137, 187, 0],
    [34, 123, 160, 222, 0],
    [4, 93, 155, 225, 0],
    [37, 113, 125, 188, 0],
    [25, 126, 166, 217, 0],
    [5, 66, 121, 134, 227],
    [38, 69, 128, 150, 197],
    [31, 100, 129, 224, 0],
    [39, 82, 130, 196, 0],
    [40, 114, 131, 195, 0],
    [23, 77, 140, 223, 0],
    [24, 133, 148, 180, 0],
    [41, 80, 144, 210, 0],
    [26, 70, 135, 143, 208],
    [42, 60, 118, 136, 213],
    [8, 38, 130, 140, 218],
    [43, 55, 138, 145, 230],
    [44, 138, 139, 225, 0],
    [45, 136, 153, 216, 0],
    [4, 83, 153, 226, 0],
    [46, 142, 177, 229, 0],
    [47, 103, 143, 228, 0],
    [35, 110, 149, 191, 0],
    [20, 96, 156, 227, 0],
    [48, 114, 157, 229, 0],
    [102, 184, 219, 234, 0],
    [5, 133, 141, 222, 0],
    [41, 84, 121, 182, 0],
    [30, 103, 150, 164, 0],
    [26, 101, 151, 227, 0],
    [36, 152, 178, 191, 0],
    [49, 83, 168, 234, 0],
    [45, 124, 154, 183, 0],
    [39, 154, 169, 232, 0],
    [33, 73, 101, 131, 200],
    [6, 128, 141, 232, 0],
    [1, 27, 158, 198, 0],
    [50, 135, 159, 224, 0],
    [35, 123, 179, 216, 0],
    [3, 112, 161, 209, 0],
    [48, 154, 162, 228, 0],
    [21, 56, 160, 163, 195],
    [7, 136, 146, 207, 0],
    [46, 66, 105, 153, 173],
    [31, 70, 113, 126, 231],
    [15, 58, 167, 176, 229],
    [18, 93, 109, 231, 0],
    [21, 74, 81, 170, 230],
    [38, 78, 166, 234, 0],
    [28, 156, 171, 181, 0],
    [29, 126, 172, 233, 0],
    [49, 51, 92, 159, 236],
    [19, 169, 174, 236, 0],
    [1, 37, 71, 128, 155],
    [129, 164, 167, 223, 0],
    [51, 108, 177, 185, 0],
    [37, 61, 152, 162, 168],
    [22, 64, 150, 163, 194],
    [52, 62, 90, 91, 210],
    [41, 107, 179, 240, 0],
    [10, 104, 115, 219, 0],
    [34, 67, 98, 142, 208],
    [43, 147, 149, 207, 0],
    [27, 68, 108, 151, 231],
    [50, 148, 189, 237, 0],
    [6, 68, 137, 142, 192],
    [46, 172, 174, 212, 0],
    [52, 78, 201, 221, 0],
    [53, 125, 176, 222, 0],
    [40, 135, 178, 238, 0],
    [43, 92, 192, 201, 0],
    [54, 79, 193, 232, 0],
    [53, 130, 156, 239, 0],
    [7, 85, 170, 240, 0],
    [55, 84, 109, 223, 0],
    [47, 171, 201, 215, 0],
    [9, 50, 149, 198, 212],
    [39, 90, 178, 213, 0],
    [25, 94, 169, 190, 0],
    [29, 179, 203, 221, 0],
    [8, 138, 202, 214, 0],
    [17, 79, 187, 235, 0],
    [49, 59, 77, 146, 205],
    [14, 75, 157, 235, 0],
    [42, 63, 155, 161, 224],
    [52, 158, 160, 218, 0],
    [45, 89, 140, 199, 0],
    [56, 95, 147, 220, 0],
    [57, 69, 97, 180, 187],
    [55, 86, 211, 238, 0],
    [2, 9, 57, 125, 184],
    [36, 76, 202, 215, 0],
    [53, 161, 166, 211, 0],
    [20, 67, 158, 193, 204],
    [12, 111, 116, 228, 0],
    [30, 89, 152, 233, 0],
    [1, 58, 134, 145, 0],
    [13, 86, 137, 226, 0],
    [44, 72, 105, 186, 220],
    [32, 106, 163, 239, 0],
    [3, 33, 80, 183, 0],
    [14, 120, 189, 202, 0],
    [59, 104, 176, 225, 0],
    [60, 110, 133, 236, 0],
    [48, 63, 87, 170, 204],
    [22, 61, 115, 171, 183],
    [19, 119, 132, 239, 0],
    [28, 88, 144, 173, 0],
    [186, 203, 205, 238, 0],
    [51, 91, 123, 211, 0],
    [32, 94, 157, 209, 0],
    [11, 58, 124, 203, 237],
    [61, 65, 122, 174, 206],
    [54, 98, 189, 217, 0],
    [47, 132, 198, 226, 0],
    [57, 81, 165, 233, 0],
    [40, 60, 134, 184, 206],
    [54, 167, 168, 240, 0],
    [3, 24, 62, 102, 177],
];

/// (240,74) row weights. ref: same file (`data nrw`).
const NRW_240_74: [u8; 166] = [5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 4, 4, 4, 5, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 5, 5, 4, 4, 4, 5, 5, 4, 5, 4, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 4, 5, 4, 5, 5, 5, 4, 5, 4, 4, 4, 5, 4, 5, 4, 4, 5, 5, 5, 4, 4, 5, 4, 5, 4, 5, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 5, 4, 5, 4, 4, 4, 5, 4, 5, 4, 4, 5, 4, 4, 4, 4, 5, 4, 4, 4, 4, 4, 5, 5, 4, 4, 4, 4, 4, 5, 5, 4, 4, 4, 5, 4, 5];

/// The FST4 (240,101) LDPC code: systematic generator (TX encode) + sparse `Nm`
/// Tanner graph (RX min-sum/OSD). ref: wsjtx/lib/fst4/ldpc_240_101_*.f90.
pub fn fst4_240_101_code() -> Ldpc {
    let p: Vec<Vec<u8>> = gen_240_101().iter().map(|r| r.to_vec()).collect();
    let mut check_vars = vec![Vec::new(); 139];
    for (c, vars) in check_vars.iter_mut().enumerate() {
        for &v in NM_240_101[c].iter().take(NRW_240_101[c] as usize) {
            vars.push(v as usize - 1);
        }
    }
    Ldpc::from_systematic_sparse(101, &p, check_vars)
}

/// The FST4W (240,74) LDPC code. ref: wsjtx/lib/fst4/ldpc_240_74_*.f90.
pub fn fst4_240_74_code() -> Ldpc {
    let p: Vec<Vec<u8>> = gen_240_74().iter().map(|r| r.to_vec()).collect();
    let mut check_vars = vec![Vec::new(); 166];
    for (c, vars) in check_vars.iter_mut().enumerate() {
        for &v in NM_240_74[c].iter().take(NRW_240_74[c] as usize) {
            vars.push(v as usize - 1);
        }
    }
    Ldpc::from_systematic_sparse(74, &p, check_vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vector from the UNMODIFIED wsjtx encoder, captured by
    /// scratch/refvectors/fst4_ldpc_dump (message bit = 1 where i % 3 == 0).
    const REF_240_101_CODEWORD: &str = "100100100100100100100100100100100100100100100100100100100100100100100100100100100100100100100100100101111111000010000101000000010100100101110000000101000110010111111111000010111000111110111111011011110111100011111100111100110011101111111011";

    #[test]
    fn encode_240_101_matches_wsjtx_reference() {
        let mut msg = [0u8; 101];
        for (i, m) in msg.iter_mut().enumerate() {
            *m = u8::from(i % 3 == 0);
        }
        let cw = encode_240_101(&msg);
        let want: Vec<u8> = REF_240_101_CODEWORD.bytes().map(|c| c - b'0').collect();
        assert_eq!(want.len(), 240, "reference codeword must be 240 bits");
        assert_eq!(cw.to_vec(), want, "FST4 (240,101) codeword differs from wsjtx");
    }


    /// Golden vector from the UNMODIFIED wsjtx encode240_74 (message bit = 1
    /// where i % 3 == 0). ref: scratch/refvectors/fst4_ldpc_dump.
    const REF_240_74_CODEWORD: &str = "100100100100100100100100100100100100100100100100100100100100100100100100101001010111000100101111010110011101001010110011101100100010100001100001001111011000100100111011010110110000101011000010000111011111111100000011101011000110111011011100";

    #[test]
    fn encode_240_74_matches_wsjtx_reference() {
        let mut msg = [0u8; 74];
        for (i, m) in msg.iter_mut().enumerate() {
            *m = u8::from(i % 3 == 0);
        }
        let cw = encode_240_74(&msg);
        let want: Vec<u8> = REF_240_74_CODEWORD.bytes().map(|c| c - b'0').collect();
        assert_eq!(want.len(), 240, "reference codeword must be 240 bits");
        assert_eq!(cw.to_vec(), want, "FST4W (240,74) codeword differs from wsjtx");
    }


    #[test]
    fn fst4_generators_and_parity_tables_agree() {
        // Strongest cross-table check: every generator codeword satisfies every
        // sparse Nm parity check (G . H^T = 0). Catches a mis-transcription of
        // either the generator or the parity table for both codes.
        for &(k, code) in &[(101usize, 0u8), (74usize, 1u8)] {
            let cw: Vec<u8>;
            let c;
            if code == 0 {
                let mut m = [0u8; 101];
                for (i, x) in m.iter_mut().enumerate() { *x = u8::from((i * 5 + 2) % 3 == 0); }
                cw = encode_240_101(&m).to_vec();
                c = fst4_240_101_code();
            } else {
                let mut m = [0u8; 74];
                for (i, x) in m.iter_mut().enumerate() { *x = u8::from((i * 5 + 2) % 3 == 0); }
                cw = encode_240_74(&m).to_vec();
                c = fst4_240_74_code();
            }
            assert_eq!(c.parity_errors(&cw), 0, "k={k}: generator and Nm disagree");
        }
    }

    #[test]
    fn fst4_240_101_bp_corrects_bit_flips() {
        let code = fst4_240_101_code();
        let mut msg = [0u8; 101];
        for (i, m) in msg.iter_mut().enumerate() { *m = u8::from(i % 2 == 0); }
        let cw = encode_240_101(&msg);
        let mut llrs: Vec<f32> = cw.iter().map(|&b| if b == 0 { 4.0 } else { -4.0 }).collect();
        for &i in &[1usize, 17, 33, 60, 99, 130, 175, 230] { llrs[i] = -llrs[i]; }
        let (hard, errs) = code.decode_minsum(&llrs, 50);
        assert_eq!(errs, 0, "BP left unsatisfied checks");
        assert_eq!(&hard[..101], &msg[..], "BP did not recover the message");
    }

    #[test]
    fn fst4_240_74_bp_corrects_bit_flips() {
        let code = fst4_240_74_code();
        let mut msg = [0u8; 74];
        for (i, m) in msg.iter_mut().enumerate() { *m = u8::from(i % 3 == 0); }
        let cw = encode_240_74(&msg);
        let mut llrs: Vec<f32> = cw.iter().map(|&b| if b == 0 { 4.0 } else { -4.0 }).collect();
        for &i in &[2usize, 20, 45, 88, 140, 200] { llrs[i] = -llrs[i]; }
        let (hard, errs) = code.decode_minsum(&llrs, 50);
        assert_eq!(errs, 0, "BP left unsatisfied checks (240,74)");
        assert_eq!(&hard[..74], &msg[..], "BP did not recover the (240,74) message");
    }

    #[test]
    fn encode_240_101_is_systematic() {
        let msg = [1u8; 101];
        let cw = encode_240_101(&msg);
        assert_eq!(&cw[..101], &msg[..], "first K bits must equal the message");
    }
}
