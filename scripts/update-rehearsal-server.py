#!/usr/bin/env python3
"""The update rehearsal's throwaway pieces (W9 plan D11, scripts/update-rehearsal.ps1): a test signing key, a certificate for the
loopback address, and the HTTPS server the installed 0.4.x app updates from. Nothing here is the real key, address or bucket.

    update-rehearsal-server.py keygen <key file>      a new Ed25519 key: the 32-byte seed in base64 to <key file> (the format
                                                      make-update-manifest.py --sign-key reads); prints the public key in base64
    update-rehearsal-server.py cert <folder>          a self-signed certificate for localhost and 127.0.0.1, valid for two days:
                                                      cert.pem, key.pem and cert.cer (DER, for the Windows certificate store)
    update-rehearsal-server.py serve <folder> --cert <pem> --key <pem> --port-file <file> --log <file>
                                                      serves <folder>'s files over HTTPS on 127.0.0.1 at a free port, written to
                                                      <port file> once listening; every request is a line in <log file>

Needs Python's `cryptography` package (keygen, cert).
"""
import argparse, base64, datetime, http.server, ipaddress, os, ssl, sys, threading


def keygen(path):
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    if os.path.exists(path):
        sys.exit(f"refusing to overwrite {path}")
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    key = Ed25519PrivateKey.generate()
    seed = key.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw, serialization.NoEncryption())
    with open(path, "w", encoding="ascii", newline="\n") as f:
        f.write(base64.b64encode(seed).decode("ascii") + "\n")
    public = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    print(base64.b64encode(public).decode("ascii"))


def cert(folder):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID
    os.makedirs(folder, exist_ok=True)
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Mighty Code update rehearsal (throwaway)")])
    now = datetime.datetime.now(datetime.timezone.utc)
    certificate = (
        x509.CertificateBuilder()
        .subject_name(name).issuer_name(name).public_key(key.public_key()).serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5)).not_valid_after(now + datetime.timedelta(days=2))
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
        .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
        .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
        .sign(key, hashes.SHA256())
    )
    with open(os.path.join(folder, "cert.pem"), "wb") as f:
        f.write(certificate.public_bytes(serialization.Encoding.PEM))
    with open(os.path.join(folder, "cert.cer"), "wb") as f:
        f.write(certificate.public_bytes(serialization.Encoding.DER))
    with open(os.path.join(folder, "key.pem"), "wb") as f:
        f.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    print(certificate.fingerprint(hashes.SHA1()).hex().upper())


def serve(folder, cert_path, key_path, port_file, log_path):
    lock = threading.Lock()
    log = open(log_path, "a", encoding="utf-8", buffering=1)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=folder, **kwargs)

        def list_directory(self, path):
            self.send_error(404)
            return None

        def log_message(self, format, *args):
            with lock:
                log.write("%s %s\n" % (datetime.datetime.now(datetime.timezone.utc).isoformat(), format % args))

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(cert_path, key_path)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    with open(port_file + ".tmp", "w", encoding="ascii") as f:
        f.write(str(server.server_address[1]))
    os.replace(port_file + ".tmp", port_file)
    server.serve_forever()


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("keygen").add_argument("key")
    commands.add_parser("cert").add_argument("folder")
    serving = commands.add_parser("serve")
    serving.add_argument("folder")
    serving.add_argument("--cert", required=True)
    serving.add_argument("--key", required=True)
    serving.add_argument("--port-file", required=True)
    serving.add_argument("--log", required=True)
    a = parser.parse_args()
    if a.command == "keygen":
        keygen(a.key)
    elif a.command == "cert":
        cert(a.folder)
    else:
        serve(os.path.abspath(a.folder), a.cert, a.key, a.port_file, a.log)


if __name__ == "__main__":
    main()
