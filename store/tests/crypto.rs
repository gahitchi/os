// SPDX-License-Identifier: GPL-3.0-only
//! The scrambling and fingerprints against the official test vectors.

use alaya_store::crypto::{Aes256, Hmac, sha256};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn aes256_fips197() {
    // FIPS 197, appendix C.3.
    let key: [u8; 32] = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
        .try_into()
        .unwrap();
    let mut b: [u8; 16] = hex("00112233445566778899aabbccddeeff").try_into().unwrap();
    Aes256::new(&key).encrypt(&mut b);
    assert_eq!(b.to_vec(), hex("8ea2b7ca516745bfeafc49904b496089"));
}

#[test]
fn aes256_ctr_sp800_38a() {
    // NIST SP 800-38A, F.5.5 (CTR-AES256.Encrypt).
    let key: [u8; 32] = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
        .try_into()
        .unwrap();
    let iv: [u8; 16] = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff").try_into().unwrap();
    let mut d = hex(concat!(
        "6bc1bee22e409f96e93d7e117393172a",
        "ae2d8a571e03ac9c9eb76fac45af8e51",
        "30c81c46a35ce411e5fbc1191a0a52ef",
        "f69f2445df4f9b17ad2b417be66c3710"
    ));
    Aes256::new(&key).ctr(&iv, &mut d);
    assert_eq!(
        d,
        hex(concat!(
            "601ec313775789a5b7a7f504bbf3d228",
            "f443e3ca4d62b59aca84e990cacaf5c5",
            "2b0930daa23de94ce87017ba2d84988d",
            "dfc9c58db67aada613c2dd08457941a6"
        ))
    );
}

#[test]
fn sha256_fips180() {
    assert_eq!(
        sha256(b"").to_vec(),
        hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
    assert_eq!(
        sha256(b"abc").to_vec(),
        hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(
        sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_vec(),
        hex("248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
    );
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        sha256(&million).to_vec(),
        hex("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
    );
}

#[test]
fn hmac_rfc4231() {
    // Test case 2.
    let m = Hmac::new(b"Jefe").mac(&[b"what do ya want ", b"for nothing?"]);
    assert_eq!(
        m.to_vec(),
        hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    // Test case 6: a key longer than a block.
    let key = vec![0xaa; 131];
    let m = Hmac::new(&key).mac(&[b"Test Using Larger Than Block-Size Key - Hash Key First"]);
    assert_eq!(
        m.to_vec(),
        hex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
    );
}
