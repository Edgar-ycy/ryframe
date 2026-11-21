use hmac::{KeyInit, Mac, SimpleHmac};
use reqwest::Url;
use sha2::{Digest, Sha256};

use super::{StorageError, StorageResult, encoded_segment};

pub(super) struct SigV4Signer<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
}

impl SigV4Signer<'_> {
    pub(super) fn authorization(
        &self,
        method: &str,
        url: &Url,
        payload_hash: &str,
        amz_date: &str,
        canned_acl: Option<&str>,
    ) -> StorageResult<String> {
        let date_stamp = amz_date
            .get(..8)
            .filter(|date| date.len() == 8 && date.as_bytes().iter().all(u8::is_ascii_digit));
        let Some(date_stamp) = date_stamp else {
            return Err(StorageError::Signing(
                "x-amz-date must start with YYYYMMDD".to_owned(),
            ));
        };
        let host = canonical_host(url)?;
        let canonical_query = canonical_query(url);
        let (canonical_acl, signed_headers) = match canned_acl {
            Some(acl) => (
                format!("x-amz-acl:{acl}\n"),
                "host;x-amz-acl;x-amz-content-sha256;x-amz-date",
            ),
            None => (String::new(), "host;x-amz-content-sha256;x-amz-date"),
        };
        let canonical_headers = format!(
            "host:{host}\n{canonical_acl}x-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n"
        );
        let canonical_request = format!(
            "{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
            url.path()
        );
        let scope = format!("{date_stamp}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );

        let date_key = hmac_sign(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let region_key = hmac_sign(&date_key, self.region.as_bytes());
        let service_key = hmac_sign(&region_key, b"s3");
        let signing_key = hmac_sign(&service_key, b"aws4_request");
        let signature = hex::encode(hmac_sign(&signing_key, string_to_sign.as_bytes()));

        Ok(format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
            self.access_key
        ))
    }
}

fn canonical_host(url: &Url) -> StorageResult<String> {
    let host = url
        .host_str()
        .ok_or_else(|| StorageError::Signing("storage endpoint has no host".to_owned()))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

fn canonical_query(url: &Url) -> String {
    let mut pairs = url
        .query_pairs()
        .map(|(key, value)| (encoded_segment(&key), encoded_segment(&value)))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        SimpleHmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_canned_bucket_acl_and_preserves_signing_without_acl() {
        let signer = SigV4Signer {
            access_key: "fixture-access",
            secret_key: "fixture-signing-secret",
            region: "us-east-1",
        };
        let url = Url::parse("http://127.0.0.1:9000/uploads?acl").unwrap();
        let payload_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        for (canned_acl, signed_headers, signature) in [
            (
                Some("private"),
                "host;x-amz-acl;x-amz-content-sha256;x-amz-date",
                "114e352c878672d1b1d84fde474e8a5287c35915f8dd5582ec50c98716fc2229",
            ),
            (
                None,
                "host;x-amz-content-sha256;x-amz-date",
                "799846f2b9269ace2861b2a2c90a48d95104e540625fdf8fac9c4d8145818f3c",
            ),
        ] {
            let authorization = signer
                .authorization("PUT", &url, payload_hash, "20261122T001100Z", canned_acl)
                .unwrap();
            assert_eq!(
                authorization,
                format!(
                    concat!(
                        "AWS4-HMAC-SHA256 Credential=fixture-access/20261122/us-east-1/s3/aws4_request,",
                        "SignedHeaders={},Signature={}"
                    ),
                    signed_headers, signature
                )
            );
        }
    }
}
