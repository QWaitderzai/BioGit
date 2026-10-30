//! 2-битное кодирование нуклеотидных последовательностей (формат UCSC .2bit).
//!
//! Основания кодируются двумя битами: A=00, C=01, G=10, T=11 —
//! один байт вмещает четыре основания. Неопределённые позиции (N)
//! кодов не имеют и помечаются отдельной битовой маской.

use crate::error::{BioGitError, Result};

/// Упакованная последовательность: коды оснований + маска N.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed2Bit {
    /// Коды оснований: 4 основания в байте, первое основание — в старших битах.
    pub data: Vec<u8>,
    /// Маска N: по одному биту на основание, старший бит первого основания в байте.
    pub n_mask: Vec<u8>,
    /// Число оснований в последовательности.
    pub len: usize,
}

const CODE_A: u8 = 0b00;
const CODE_C: u8 = 0b01;
const CODE_G: u8 = 0b10;
const CODE_T: u8 = 0b11;

fn base_to_code(base: u8) -> Result<u8> {
    match base {
        b'A' | b'a' => Ok(CODE_A),
        b'C' | b'c' => Ok(CODE_C),
        b'G' | b'g' => Ok(CODE_G),
        b'T' | b't' => Ok(CODE_T),
        b'N' | b'n' => Ok(CODE_A), // код-заглушка, позиция помечается в маске N
        other => Err(BioGitError::InvalidBase(other as char)),
    }
}

fn code_to_base(code: u8, is_n: bool) -> u8 {
    if is_n {
        return b'N';
    }
    match code {
        CODE_A => b'A',
        CODE_C => b'C',
        CODE_G => b'G',
        _ => b'T',
    }
}

/// Упаковывает ASCII-последовательность ДНК (A/C/G/T/N) в 2-битный формат.
///
/// # Примеры
///
/// ```
/// use biogit_core::encoding::{pack_two_bit, unpack_two_bit};
///
/// let seq = b"ATCGATCGNNNATCG";
/// let packed = pack_two_bit(seq).unwrap();
/// assert_eq!(packed.data.len(), 4);           // 15 оснований -> 4 байта
/// assert_eq!(unpack_two_bit(&packed), seq.to_vec());
/// ```
pub fn pack_two_bit(seq: &[u8]) -> Result<Packed2Bit> {
    let mut packed = Packed2Bit {
        data: vec![0u8; seq.len().div_ceil(4)],
        n_mask: vec![0u8; seq.len().div_ceil(8)],
        len: seq.len(),
    };
    for (i, &base) in seq.iter().enumerate() {
        let code = base_to_code(base)?;
        let shift = (3 - (i % 4)) * 2;
        packed.data[i / 4] |= code << shift;
        if matches!(base, b'N' | b'n') {
            packed.n_mask[i / 8] |= 1u8 << (7 - (i % 8));
        }
    }
    Ok(packed)
}

/// Распаковывает 2-битный формат обратно в ASCII-последовательность.
pub fn unpack_two_bit(packed: &Packed2Bit) -> Vec<u8> {
    let mut out = Vec::with_capacity(packed.len);
    for i in 0..packed.len {
        let shift = (3 - (i % 4)) * 2;
        let code = (packed.data[i / 4] >> shift) & 0b11;
        let is_n = packed.n_mask[i / 8] & (1u8 << (7 - (i % 8))) != 0;
        out.push(code_to_base(code, is_n));
    }
    out
}

/// Обратный комплемент: реверс последовательности + побитовый NOT каждого кода.
///
/// Побитовый NOT двухбитного кода даёт комплементарное основание:
/// NOT(00)=11 (A→T), NOT(01)=10 (C→G), NOT(10)=01 (G→C), NOT(11)=00 (T→A).
/// Позиции N отслеживаются маской и остаются N.
pub fn reverse_complement(seq: &[u8]) -> Result<Vec<u8>> {
    let packed = pack_two_bit(seq)?;
    let mut out = Vec::with_capacity(packed.len);
    for i in (0..packed.len).rev() {
        let shift = (3 - (i % 4)) * 2;
        let code = (packed.data[i / 4] >> shift) & 0b11;
        let is_n = packed.n_mask[i / 8] & (1u8 << (7 - (i % 8))) != 0;
        if is_n {
            out.push(b'N');
        } else {
            out.push(code_to_base(!code & 0b11, false)); // NOT == комплемент
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_пробной_последовательности() {
        let seq = b"ATCGATCGNNNATCG";
        let packed = pack_two_bit(seq).expect("упаковка должна пройти успешно");
        assert_eq!(packed.len, 15);
        assert_eq!(packed.data.len(), 4); // ceil(15/4)
        assert_eq!(packed.n_mask.len(), 2); // ceil(15/8)
        assert_eq!(unpack_two_bit(&packed), seq.to_vec());
    }

    #[test]
    fn маска_n_отмечает_только_неопределённые_позиции() {
        let seq = b"ATCGATCGNNNATCG"; // N на позициях 8, 9, 10
        let packed = pack_two_bit(seq).expect("упаковка");
        assert_eq!(packed.n_mask[1] & 0b1110_0000, 0b1110_0000); // биты 8..10
        assert_eq!(packed.n_mask[0], 0); // первые 8 оснований без N
    }

    #[test]
    fn roundtrip_всех_длин() {
        let alphabet = b"ACGTN";
        for len in [1usize, 2, 3, 4, 5, 7, 8, 9, 16, 33] {
            let seq: Vec<u8> = (0..len).map(|i| alphabet[i % alphabet.len()]).collect();
            let packed = pack_two_bit(&seq).expect("упаковка");
            assert_eq!(unpack_two_bit(&packed), seq, "длина {len}");
        }
    }

    #[test]
    fn обратный_комплемент_простых_последовательностей() {
        assert_eq!(reverse_complement(b"ATCG").expect("rc"), b"CGAT".to_vec());
        assert_eq!(reverse_complement(b"A").expect("rc"), b"T".to_vec());
        assert_eq!(reverse_complement(b"ATCNN").expect("rc"), b"NNGAT".to_vec());
    }

    #[test]
    fn двойной_обратный_комплемент_возвращает_исходник() {
        for seq in [b"ATCGATCGNNNATCG".as_slice(), b"GGCCTAANNTTGGCC".as_slice()] {
            let rc = reverse_complement(seq).expect("rc");
            let back = reverse_complement(&rc).expect("rc");
            assert_eq!(back, seq.to_vec());
        }
    }

    #[test]
    fn регистр_не_важен() {
        assert_eq!(pack_two_bit(b"atcg").expect("упаковка"), pack_two_bit(b"ATCG").expect("упаковка"));
    }

    #[test]
    fn посторонний_символ_даёт_ошибку() {
        let err = pack_two_bit(b"ATXG").expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::InvalidBase('X')));
    }

    #[test]
    fn пустая_последовательность() {
        let packed = pack_two_bit(b"").expect("упаковка");
        assert!(packed.data.is_empty());
        assert!(unpack_two_bit(&packed).is_empty());
    }
}
