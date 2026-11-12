"""Справочник ключей сквозного шифрования bs-server (у оригинала его нет)."""
import sys, random; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from lib import http
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 3106
fails = []
def check(name, got, want):
    ok = got[0] == want.get('status', got[0]) and all(got[1].get(k) == v for k, v in want.items() if k != 'status')
    print(('✓ ' if ok else '✗ ') + name + ('' if ok else f'  получено {got}'))
    if not ok: fails.append(name)
tag = ''.join(random.choice('abcdefgh') for _ in range(8))
a = http(PORT, 'POST', '/api/auth/register', {'username': 'ea' + tag, 'password': 'password123'})[1]
b = http(PORT, 'POST', '/api/auth/register', {'username': 'eb' + tag, 'password': 'password123'})[1]
ta, tb, ida = a['token'], b['token'], a['user']['id']
KEY = 'hSDwCYkwp1R0i33ctD73Wg2_Og0mOBr066SpjqqbTmo'; KEY2 = '3p7bfXt9wbTTW2HC7OQ1Nz-DQ8hbeGdNrfx-FG-IK08'
check('ключа ещё нет', http(PORT, 'GET', '/api/e2ee/key', None, ta), {'status': 200, 'publicKey': None})
check('чужого ключа нет', http(PORT, 'GET', f'/api/e2ee/keys/{ida}', None, tb), {'status': 404, 'code': 'e2ee_key_not_found'})
check('без входа', http(PORT, 'GET', f'/api/e2ee/keys/{ida}'), {'status': 401})
for bad in [None, '', 'short', KEY + 'A', KEY[:-1] + 'p', KEY.replace('_', '/'), 5]:
    check(f'негодный ключ {bad!r}', http(PORT, 'PUT', '/api/e2ee/key', {'publicKey': bad}, ta), {'status': 400, 'code': 'e2ee_key_invalid'})
check('публикация', http(PORT, 'PUT', '/api/e2ee/key', {'publicKey': KEY}, ta), {'status': 200, 'publicKey': KEY, 'userId': ida})
check('свой ключ', http(PORT, 'GET', '/api/e2ee/key', None, ta), {'publicKey': KEY})
check('ключ виден собеседнику', http(PORT, 'GET', f'/api/e2ee/keys/{ida}', None, tb), {'status': 200, 'publicKey': KEY})
check('замена', http(PORT, 'PUT', '/api/e2ee/key', {'publicKey': KEY2}, ta), {'status': 200, 'publicKey': KEY2})
check('собеседник видит новый', http(PORT, 'GET', f'/api/e2ee/keys/{ida}', None, tb), {'publicKey': KEY2})
# зашифрованное сообщение — обычный текст для сервера и не находится поиском
dm = http(PORT, 'POST', '/api/dm', {'userId': b['user']['id']}, ta)[1]['id']
sealed = 'bs-e2ee:1:' + 'QUJD' * 20
check('сообщение принято как текст', http(PORT, 'POST', f'/api/dm/{dm}/messages', {'content': sealed}, ta), {'status': 201, 'content': sealed})
http(PORT, 'POST', f'/api/dm/{dm}/messages', {'content': 'открытый QUJD'}, ta)
r = http(PORT, 'GET', f'/api/dm/{dm}/search?q=QUJD', None, tb)
check('поиск не находит шифротекст', (r[0], {'n': r[1].get('totalCount')}), {'status': 200, 'n': 1})
check('отзыв ключа', http(PORT, 'DELETE', '/api/e2ee/key', None, ta), {'status': 200, 'success': True})
check('после отзыва ключа нет', http(PORT, 'GET', f'/api/e2ee/keys/{ida}', None, tb), {'status': 404})
print('справочник ключей: расхождений', len(fails)); sys.exit(1 if fails else 0)
