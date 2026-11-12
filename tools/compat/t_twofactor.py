"""Двухфакторная аутентификация bs-server от начала до конца (у оригинала её нет)."""
import sys, time, hmac, hashlib, base64, struct, random; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from lib import http
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 3106
def totp(secret, shift=0):
    key = base64.b32decode(secret + '=' * (-len(secret) % 8))
    digest = hmac.new(key, struct.pack('>Q', int(time.time()) // 30 + shift), hashlib.sha1).digest()
    at = digest[19] & 15
    return '%06d' % ((struct.unpack('>I', digest[at:at + 4])[0] & 0x7fffffff) % 1000000)
fails = []
def check(name, got, want):
    ok = all(got[1].get(k) == v for k, v in want.items() if k != 'status') and got[0] == want.get('status', got[0])
    print(('✓ ' if ok else '✗ ') + name + ('' if ok else f'  получено {got}'))
    if not ok: fails.append(name)
name = 'tf' + ''.join(random.choice('abcdefgh') for _ in range(8)); pw = 'password123'
reg = http(PORT, 'POST', '/api/auth/register', {'username': name, 'password': pw})[1]; tok = reg['token']
check('состояние: выключено', http(PORT, 'GET', '/api/auth/2fa', None, tok), {'status': 200, 'enabled': False, 'pending': False})
check('без входа нельзя', http(PORT, 'GET', '/api/auth/2fa'), {'status': 401})
check('настройка без пароля', http(PORT, 'POST', '/api/auth/2fa/setup', {}, tok), {'status': 400, 'code': 'password_required'})
check('настройка с чужим паролем', http(PORT, 'POST', '/api/auth/2fa/setup', {'password': 'wrong-password'}, tok), {'status': 403})
check('включение без настройки', http(PORT, 'POST', '/api/auth/2fa/enable', {'code': '123456'}, tok), {'status': 400, 'code': 'two_factor_setup_missing'})
s = http(PORT, 'POST', '/api/auth/2fa/setup', {'password': pw}, tok); secret = s[1].get('secret', '')
check('настройка', s, {'status': 200}); assert s[1]['otpauthUrl'].startswith('otpauth://totp/') and secret in s[1]['otpauthUrl']
check('состояние: ожидает', http(PORT, 'GET', '/api/auth/2fa', None, tok), {'enabled': False, 'pending': True})
check('вход до подтверждения — без кода', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw}), {'status': 200})
check('неверный код', http(PORT, 'POST', '/api/auth/2fa/enable', {'code': '000000'}, tok), {'status': 400, 'code': 'two_factor_invalid'})
e = http(PORT, 'POST', '/api/auth/2fa/enable', {'code': totp(secret)}, tok); codes = e[1].get('recoveryCodes', [])
check('включение', e, {'status': 200, 'enabled': True}); assert len(codes) == 10 and len(set(codes)) == 10
check('повторное включение', http(PORT, 'POST', '/api/auth/2fa/enable', {'code': totp(secret)}, tok), {'status': 409, 'code': 'two_factor_already_enabled'})
check('повторная настройка', http(PORT, 'POST', '/api/auth/2fa/setup', {'password': pw}, tok), {'status': 409})
check('состояние: включено', http(PORT, 'GET', '/api/auth/2fa', None, tok), {'enabled': True, 'recoveryCodesLeft': 10})
check('старый токен действует', http(PORT, 'GET', '/api/users/@me', None, tok), {'status': 200})
check('вход без кода', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw}), {'status': 401, 'code': 'two_factor_required'})
check('неверный пароль не выдаёт защиту', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': 'wrong-password'}), {'status': 401, 'code': 'invalid_credentials'})
check('вход с неверным кодом', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'totp': '000000'}), {'status': 401, 'code': 'two_factor_invalid'})
check('использованный код повторно', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'totp': totp(secret)}), {'status': 401, 'code': 'two_factor_invalid'})
check('вход со следующим кодом', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'totp': totp(secret, 1)}), {'status': 200})
check('вход с запасным кодом', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'recoveryCode': codes[0].upper().replace('-', ' ')}), {'status': 200})
for _ in range(4): http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'totp': '000000'})
check('предел неверных кодов', http(PORT, 'POST', '/api/auth/login', {'username': name, 'password': pw, 'totp': totp(secret, 2)}), {'status': 429, 'code': 'two_factor_rate_limited'})
name2 = name + 'b'; tok2 = http(PORT, 'POST', '/api/auth/register', {'username': name2, 'password': pw})[1]['token']
sec2 = http(PORT, 'POST', '/api/auth/2fa/setup', {'password': pw}, tok2)[1]['secret']
codes2 = http(PORT, 'POST', '/api/auth/2fa/enable', {'code': totp(sec2)}, tok2)[1]['recoveryCodes']
check('запасной код одноразовый: первый раз', http(PORT, 'POST', '/api/auth/login', {'username': name2, 'password': pw, 'recoveryCode': codes2[3]}), {'status': 200})
check('запасной код одноразовый: второй раз', http(PORT, 'POST', '/api/auth/login', {'username': name2, 'password': pw, 'recoveryCode': codes2[3]}), {'status': 401, 'code': 'two_factor_invalid'})
check('осталось кодов', http(PORT, 'GET', '/api/auth/2fa', None, tok2), {'recoveryCodesLeft': 9})
r = http(PORT, 'POST', '/api/auth/2fa/recovery-codes', {'password': pw, 'code': totp(sec2, 1)}, tok2)
check('новые запасные коды', r, {'status': 200}); assert len(r[1]['recoveryCodes']) == 10 and codes2[0] not in r[1]['recoveryCodes']
check('отключение без пароля', http(PORT, 'POST', '/api/auth/2fa/disable', {'recoveryCode': r[1]['recoveryCodes'][0]}, tok2), {'status': 400, 'code': 'password_required'})
check('отключение', http(PORT, 'POST', '/api/auth/2fa/disable', {'password': pw, 'recoveryCode': r[1]['recoveryCodes'][0]}, tok2), {'status': 200, 'enabled': False})
check('вход без кода снова работает', http(PORT, 'POST', '/api/auth/login', {'username': name2, 'password': pw}), {'status': 200})
check('отключать нечего', http(PORT, 'POST', '/api/auth/2fa/disable', {'password': pw, 'code': '123456'}, tok2), {'status': 400, 'code': 'two_factor_not_enabled'})
print('двухфакторка: расхождений', len(fails)); sys.exit(1 if fails else 0)
