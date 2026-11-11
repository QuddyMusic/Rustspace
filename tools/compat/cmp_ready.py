import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from lib import *
bad=0
for name in sys.argv[1:] or ['alice','bob','carol']:
    toks=[http(p,'POST','/api/auth/login',{'username':name,'password':'password123'})[1]['token'] for p in (3105,3106)]
    a=norm(ready(3105,toks[0])); b=norm(ready(3106,toks[1]))
    d=diff(a,b)
    print(name, 'OK' if not d else f'{len(d)} расхождений'); bad+=len(d)
    for line in d[:40]: print('  ',line)
sys.exit(1 if bad else 0)
