// Copyright (C) 2019-2020  Pierre Krieger
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Generating cryptographically-secure random data.

#![deny(intra_doc_link_resolution_failure)]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use byteorder::{ByteOrder as _, NativeEndian};
use core::convert::TryFrom as _;

pub mod ffi;

/// Generate `len` bytes of random data and returns them.
#[cfg(feature = "std")]
pub async fn generate(len: usize) -> Vec<u8> {
    unsafe {
        let mut out = Vec::with_capacity(len);
        out.set_len(len);
        generate_in(&mut out).await;
        out
    }
}

/// Fills `out` with randomly-generated data.
#[cfg(feature = "std")]
pub async fn generate_in(out: &mut [u8]) {
    for chunk in out.chunks_mut(usize::from(u16::max_value())) {
        let msg = ffi::RandomMessage::Generate {
            len: u16::try_from(chunk.len()).unwrap(),
        };
        let rep: ffi::GenerateResponse = unsafe {
            redshirt_syscalls::emit_message_with_response(&ffi::INTERFACE, msg)
                .unwrap()
                .await
        };
        chunk.copy_from_slice(&rep.result);
    }
}

/// Generates a random `u8`.
#[cfg(feature = "std")]
pub async fn generate_u8() -> u8 {
    let mut buf = [0; 1];
    generate_in(&mut buf).await;
    buf[0]
}

/// Generates a random `u16`.
#[cfg(feature = "std")]
pub async fn generate_u16() -> u16 {
    let mut buf = [0; 2];
    generate_in(&mut buf).await;
    NativeEndian::read_u16(&buf)
}

/// Generates a random `u32`.
#[cfg(feature = "std")]
pub async fn generate_u32() -> u32 {
    let mut buf = [0; 4];
    generate_in(&mut buf).await;
    NativeEndian::read_u32(&buf)
}

/// Generates a random `u64`.
#[cfg(feature = "std")]
pub async fn generate_u64() -> u64 {
    let mut buf = [0; 8];
    generate_in(&mut buf).await;
    NativeEndian::read_u64(&buf)
}

/// Generates a random `u128`.
#[cfg(feature = "std")]
pub async fn generate_u128() -> u128 {
    let mut buf = [0; 16];
    generate_in(&mut buf).await;
    NativeEndian::read_u128(&buf)
}
