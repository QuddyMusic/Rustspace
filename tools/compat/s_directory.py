import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
from lib import http
def directory(c):
    c.user('al')
    root=http(c.port,'POST','/api/auth/login',{'username':'alice','password':'password123'})[1]['token']; c.tok['root']=root
    pub=c.call('al','POST','/api/spaces',{'name':'В каталоге','avatarColor':'sky'})['id']
    c.call('al','PATCH',f'/api/spaces/{pub}',{'visibility':'public','description':'о'*250,'directoryListed':True})
    c.call(None,'GET','/api/directory/spaces')
    c.call('root','PATCH','/api/settings/instance',{'directoryEnabled':True,'discoveryEnabled':True,'directoryBrowseEnabled':True})
    c.call(None,'GET','/api/directory/spaces')
    c.raw(None,'GET','/api/directory/spaces',keep=('cache-control',),auth=False)
    c.call('al','GET','/api/directory'); c.call(None,'GET','/api/directory'); c.call('al','GET','/api/directory?q=x&limit=5&offset=2')
    c.raw(None,'POST','/api/csp-report',{'Content-Type':'application/csp-report'},b'{"csp-report":{"blocked-uri":"x"}}',auth=False)
    c.raw(None,'POST','/api/csp-report',{'Content-Type':'application/reports+json'},b'not json\n[fake] line',auth=False)
    c.raw(None,'POST','/api/csp-report',{'Content-Type':'application/json'},b'{}',auth=False)

if __name__=='__main__': sys.exit(1 if run(directory, show='-v' in sys.argv) else 0)
