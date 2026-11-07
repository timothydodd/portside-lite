//! Backup files sealed with a passphrase. The format is plain
//! [age](https://age-encryption.org) (scrypt recipient), so a backup can be
//! opened without Portside: `age -d secrets.tar.gz.age | tar xz`.
//!
//! A set of small files is sealed as one `.tar.gz` (one passphrase prompt,
//! one key derivation); large ones (a datastore snapshot) are streamed
//! through [`seal_writer`].

use std::io::{self, Read, Write};

use age::secrecy::SecretString;

/// File name ending for anything sealed here.
pub const SUFFIX: &str = ".age";

fn secret(passphrase: &str) -> SecretString {
    SecretString::from(passphrase.to_owned())
}

/// A writer that encrypts everything written to it into `out`. Call
/// `finish()` on it at the end, or the file is cut short and won't open.
pub fn seal_writer<W: Write>(out: W, passphrase: &str) -> io::Result<age::stream::StreamWriter<W>> {
    if passphrase.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no backup passphrase"));
    }
    age::Encryptor::with_user_passphrase(secret(passphrase)).wrap_output(out)
}

/// A reader that decrypts `input`. A wrong passphrase fails here, before
/// anything is read.
pub fn open_reader<R: Read>(input: R, passphrase: &str) -> Result<impl Read, String> {
    let decryptor = age::Decryptor::new(input).map_err(open_error)?;
    if !decryptor.is_scrypt() {
        return Err("This file wasn't sealed with a passphrase.".into());
    }
    let identity = age::scrypt::Identity::new(secret(passphrase));
    decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)).map_err(open_error)
}

fn open_error(e: age::DecryptError) -> String {
    match e {
        age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys => "Wrong backup passphrase.".into(),
        e => format!("Couldn't open the sealed file: {e}"),
    }
}

/// Seal `files` (relative path, contents) as one encrypted `.tar.gz`.
pub fn seal_files(files: &[(String, Vec<u8>)], passphrase: &str) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let writer = seal_writer(&mut out, passphrase)?;
    let gz = flate2::write::GzEncoder::new(writer, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (path, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(0);
        header.set_cksum();
        tar.append_data(&mut header, path, data.as_slice())?;
    }
    tar.into_inner()?.finish()?.finish()?;
    Ok(out)
}

/// Open what [`seal_files`] made: every regular file, by its path. Paths
/// that would leave the folder (absolute, `..`) are refused.
pub fn open_files(sealed: &[u8], passphrase: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let reader = open_reader(sealed, passphrase)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    let mut out = Vec::new();
    let damaged = |e: io::Error| format!("The sealed file is damaged: {e}");
    for entry in tar.entries().map_err(damaged)? {
        let mut entry = entry.map_err(damaged)?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(damaged)?.to_string_lossy().replace('\\', "/");
        if path.starts_with('/') || path.split('/').any(|s| s == "..") {
            return Err(format!("The sealed file holds an unsafe path: {path}"));
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(damaged)?;
        out.push((path, data));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_a_wrong_passphrase_fails() {
        let files = vec![("secrets/Secret.yaml".to_string(), b"kind: Secret\n".to_vec()), ("node/registries.yaml".to_string(), vec![0, 1, 2])];
        let sealed = seal_files(&files, "correct horse").unwrap();
        assert!(sealed.starts_with(b"age-encryption.org/v1"));
        assert_eq!(open_files(&sealed, "correct horse").unwrap(), files);
        assert_eq!(open_files(&sealed, "wrong").unwrap_err(), "Wrong backup passphrase.");
        assert!(seal_files(&files, "").is_err(), "no passphrase, no file");
    }

    #[test]
    fn streams_round_trip() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        let mut w = seal_writer(&mut out, "pw").unwrap();
        w.write_all(&data).unwrap();
        w.finish().unwrap();
        let mut back = Vec::new();
        open_reader(out.as_slice(), "pw").unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
    }
}
