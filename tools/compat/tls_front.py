"""TLS-фронт стенда федерации: принимает https на 127.0.0.1:443 и по имени
узла (SNI) передаёт соединение инстансу на его порт по http.

Федерация оригинала адресует инстанс именем узла без порта (`https://домен`),
поэтому пары инстансов на одном компьютере различаются именами: x1.test,
y1.test, x2.test, y2.test (см. fed_up.sh). Сертификат подписан тестовым
удостоверяющим центром стенда; Node.js доверяет ему через
NODE_EXTRA_CA_CERTS, bs-server — через BS_EXTRA_CA_CERTS.

Запуск: python3 tls_front.py <каталог-сертификатов> имя=порт ...
"""
import asyncio, os, ssl, sys

def main():
    certs, routes = sys.argv[1], dict(arg.split('=') for arg in sys.argv[2:])
    names = {}
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(f'{certs}/front.pem', f'{certs}/front.key')
    def remember(sslobj, name, _ctx):
        names[id(sslobj)] = name
    ctx.sni_callback = remember

    async def pipe(reader, writer):
        try:
            while data := await reader.read(65536):
                writer.write(data)
                await writer.drain()
        except Exception:
            pass
        finally:
            try: writer.close()
            except Exception: pass

    active = {}

    async def watch():
        # Уже открытые соединения (Node.js держит их) рвутся, когда инстанс
        # объявлен недоступным.
        while True:
            await asyncio.sleep(0.1)
            for name, conns in list(active.items()):
                if os.path.exists(f'{certs}/down/{name}'):
                    for conn in list(conns):
                        for w in conn:
                            try: w.close()
                            except Exception: pass
                    conns.clear()

    async def handle(reader, writer):
        sslobj = writer.get_extra_info('ssl_object')
        name = names.pop(id(sslobj), None)
        port = routes.get(name or '')
        # Файл `<каталог>/down/<имя>` делает инстанс недоступным для других.
        if port is None or os.path.exists(f'{certs}/down/{name}'):
            writer.close(); return
        try:
            up_reader, up_writer = await asyncio.open_connection('127.0.0.1', int(port))
        except OSError:
            writer.close(); return
        conn = (writer, up_writer)
        active.setdefault(name, set()).add(conn)
        try:
            await asyncio.gather(pipe(reader, up_writer), pipe(up_reader, writer))
        finally:
            active.get(name, set()).discard(conn)

    async def serve():
        server = await asyncio.start_server(handle, '127.0.0.1', 443, ssl=ctx)
        asyncio.create_task(watch())
        async with server:
            await server.serve_forever()
    asyncio.run(serve())

if __name__ == '__main__':
    main()
