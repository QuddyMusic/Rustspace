import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
# Тело запроса, которое не разбирается как JSON: Fastify отвечает текстом
# ошибки V8 (JSON.parse) или secure-json-parse; пустое тело — своим кодом.
BODIES = ['{oops', '{"a":1,}', '[1', 'nul', '{"a" 1}', '"x', '{"a":01}', '{"a":1} x', 'tru e', '[1,2,3,4,5,6,7,8,9,10,11,12 x]',
          '{"username":"al","password":"é😀\\q"}', '{\r\n"a":\n-}', '1e', '[.5]', 'NaN', '', '   ',
          '{"__proto__":{"admin":true}}', '{"a":{"constructor":{"prototype":{}}}}', '{"constructor":{"x":1}}', '{"_\\u005fproto__":1}']
def bodies(c):
    c.user('al')
    for data in BODIES:
        c.raw(None, 'POST', '/api/auth/login', {'Content-Type': 'application/json'}, data.encode(), label=f'login {data!r}')
    for data in BODIES[:4] + BODIES[-4:]:
        c.raw('al', 'PATCH', '/api/users/me', {'Content-Type': 'application/json'}, data.encode(), label=f'profile {data!r}')
    c.raw('al', 'PATCH', '/api/users/me', {'Content-Type': 'application/json; charset=utf-8'}, b'{"displayName":"\\ud83d\\ude00 ok"}', label='profile: экранированная пара')
    c.call('al', 'GET', '/api/users/me')
    # Тип содержимого выбирает разборщик: неподходящий даёт 415 (и раньше проверки входа).
    login = '{"username":"x","password":"y"}'.encode()
    for ct in ['text/plain', 'text/plain; charset=utf-8', 'application/x-www-form-urlencoded', 'application/json-patch+json',
               'Application/Json', 'application/jsonx', 'x-application/json', 'application/vnd.api+json', 'text/html',
               'application/json; charset', 'application/json; charset="utf-8"', 'text/plain;application/json', None]:
        h = {'Content-Type': ct} if ct is not None else {}
        c.raw(None, 'POST', '/api/auth/login', h, login, label=f'login как {ct}')
        c.raw(None, 'POST', '/api/spaces', h, login, label=f'spaces без входа как {ct}')
        c.raw(None, 'POST', '/api/nothing', h, b'{x', label=f'нет маршрута, {ct}')
    c.raw(None, 'POST', '/api/spaces', {'Content-Type': 'application/json'}, b'{x', label='spaces без входа, не JSON')
    c.raw(None, 'POST', '/api/auth/login', {'Content-Type': 'text/plain'}, b'', label='login: пустой текст')
    c.raw(None, 'POST', '/api/auth/login', {'Content-Type': 'application/xml', 'Content-Length': '0'}, None, label='login: пустой xml')
    for method in ['PATCH', 'PUT', 'DELETE']:
        c.raw('al', method, '/api/users/me', {'Content-Type': 'application/json'}, b'{}', label=f'{method} /api/users/me')
        c.raw('al', method, '/api/users/me', {'Content-Type': 'application/json'}, b'{x', label=f'{method} /api/users/me не JSON')
    c.raw('al', 'DELETE', '/api/nothing', {'Content-Type': 'application/xml'}, b'{x', label='DELETE нет маршрута, xml')
    c.raw('al', 'GET', '/api/nothing', {'Content-Type': 'application/json'}, b'{x', label='GET нет маршрута, не JSON')

if __name__=='__main__': sys.exit(1 if run(bodies, show='-v' in sys.argv) else 0)
