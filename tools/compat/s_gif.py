import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
def gif(c):
    c.user('al')
    # Ключ Klipy не задан: поиск выключен, наружу запросов нет.
    for path in ['/api/gif/enabled','/api/gif/trending','/api/gif/trending?limit=5&pos=2','/api/gif/search','/api/gif/search?q=cat','/api/gif/search?q=%20']:
        c.call('al','GET',path)
    c.call(None,'GET','/api/gif/enabled'); c.call(None,'GET','/api/gif/search?q=cat')

if __name__=='__main__': sys.exit(1 if run(gif, show='-v' in sys.argv) else 0)
