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
        let canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
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
    fn signature_is_deterministic_and_scoped_to_region() {
        let signer = SigV4Signer {
            access_key: "AKIDEXAMPLE",
            secret_key: "secret",
            region: "eu-west-1",
        };
        let url =
            Url::parse("https://storage.example.com/photos/photo%20one.jpg?versionId=1").unwrap();
        let authorization = signer
            .authorization("GET", &url, "UNSIGNED-PAYLOAD", "20260716T010203Z")
            .unwrap();

        assert!(authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260716/eu-west-1/s3/aws4_request,"
        ));
        assert!(
            authorization.ends_with(
                "Signature=606edd943de076c48c13c011b74740f0088f68bd7100788a200295bd5b1419dc"
            ),
            "unexpected authorization: {authorization}"
        );
    }

    #[test]
    fn empty_query_values_are_canonicalized_with_equals() {
        let url = Url::parse("https://storage.example.com/photos?policy").unwrap();
        assert_eq!(canonical_query(&url), "policy=");
    }
}
