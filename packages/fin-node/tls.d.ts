// Self-signed certificates and QR codes (5B Task 3, Decisions 8, 17).

/**
 * A self-signed certificate (PEM) made from the identity key (`identityPem`, PKCS#8): subject
 * `CN=finisher`, SANs `sans` plus `localhost`, `127.0.0.1`, `::1`, valid from a day ago for
 * `notAfterDays` (1–397). Same key, same SPKI pin. Throws `INVALID_REQUEST`.
 */
export declare function selfSignedCert(identityPem: string, sans: string[], notAfterDays: number): string;
/** The QR code of `text` (error correction M) as an SVG document. */
export declare function qrSvg(text: string): string;
/** The QR code of `text` as Unicode half blocks, two modules a character, with the quiet zone. */
export declare function qrText(text: string): string;
