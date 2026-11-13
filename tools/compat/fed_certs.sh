#!/bin/sh
# Тестовый удостоверяющий центр и сертификат TLS-фронта стенда федерации.
# Имена x1.test, y1.test, x2.test, y2.test прописываются в /etc/hosts.
DIR=${1:-/tmp/fedca}
[ -f "$DIR/front.pem" ] && exit 0
mkdir -p "$DIR" && cd "$DIR" || exit 1
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 3650 -subj "/CN=bs-rs federation stand CA" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout front.key -out front.csr -subj "/CN=x1.test" 2>/dev/null
printf 'subjectAltName=DNS:x1.test,DNS:y1.test,DNS:x2.test,DNS:y2.test\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n' > ext.cnf
openssl x509 -req -in front.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out front.pem -days 3650 -extfile ext.cnf 2>/dev/null
grep -q 'x1.test' /etc/hosts || echo '127.0.0.1 x1.test y1.test x2.test y2.test' >> /etc/hosts
