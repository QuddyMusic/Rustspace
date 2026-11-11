import sys; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
from lib import http
def admin(c):
    a=c.user('al')
    # администратор — первый пользователь стенда
    s,b=http(c.port,'POST','/api/auth/login',{'username':'alice','password':'password123'}); c.tok['root']=b['token']; c.n.tok(b['user']['id'])
    sp=c.call('al','POST','/api/spaces',{'name':'У','avatarColor':'sky'}); sid=sp['id']
    full=c.call('al','GET',f'/api/spaces/{sid}'); gen=[x for x in full['channels'] if x['type']=='text'][0]['id']
    c.call('al','GET','/api/users/@me/notification-settings')
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{})
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'level':'loud'})
    c.call('al','PATCH','/api/spaces/nope/notification-settings',{'level':'all'})
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'level':'all','suppressEveryone':True}); c.settle()
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'mute':'indefinite'}); c.settle()
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'suppressRoles':True})
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'level':'nothing'}); c.settle()
    c.call('al','PATCH','/api/channels/nope/notification-settings',{'level':'all'})
    c.call('al','GET','/api/users/@me/notification-settings')
    c.call('al','PATCH',f'/api/spaces/{sid}/notification-settings',{'mute':None,'level':None,'suppressEveryone':False}); c.settle()
    c.call('al','PATCH',f'/api/channels/{gen}/notification-settings',{'level':None}); c.settle()
    c.call('al','GET','/api/users/@me/notification-settings')
    # приглашения
    c.call('al','GET','/api/admin/invites')
    c.call('al','POST','/api/admin/invites',{'name':'x','maxUses':None,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'','maxUses':None,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'x','maxUses':0,'expiresAt':None})
    c.call('root','POST','/api/admin/invites',{'name':'x','maxUses':None,'expiresAt':5})
    inv=c.call('root','POST','/api/admin/invites',{'name':' Друзьям '+c.tag,'maxUses':1,'expiresAt':None})
    iid=inv['id']
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'name':'Новое '+c.tag,'maxUses':2})
    c.call('root','PATCH','/api/admin/invites/nope',{'name':'x'})
    c.call(None,'GET',f"/api/auth/check-invite?token={inv['token']}",label='check-invite')
    c.call(None,'POST','/api/auth/register',{'username':'inv'+c.tag,'password':'password123','inviteToken':inv['token']},label='register-with-invite')
    c.call('root','GET',f'/api/admin/invites/{iid}/redemptions')
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'maxUses':1})
    c.call('root','POST',f'/api/admin/invites/{iid}/reinstate',{})
    c.call('root','POST',f'/api/admin/invites/{iid}/revoke')
    c.call('root','POST',f'/api/admin/invites/{iid}/revoke')
    c.call('root','PATCH',f'/api/admin/invites/{iid}',{'name':'y'})
    c.call('root','POST',f'/api/admin/invites/{iid}/reinstate',{'maxUses':5})
    c.call('root','DELETE',f'/api/admin/invites/{iid}')
    c.call('root','DELETE',f'/api/admin/invites/{iid}')
def settings(c):
    a=c.user('al')
    s,b=http(c.port,'POST','/api/auth/login',{'username':'alice','password':'password123'}); c.tok['root']=b['token']; c.n.tok(b['user']['id'])
    c.call('al','GET','/api/settings/streaming')
    c.call('al','PATCH','/api/settings/streaming',{'maxBitrateKbps':9000})
    c.call('al','GET','/api/settings/instance')
    c.call('root','GET','/api/settings/instance')
    for body in [{'maxBitrateKbps':1},{'minBitrateKbps':'x'},{'bitrateStepKbps':1},{'allowedResolutions':[]},{'allowedResolutions':[123,'x']},
                 {'allowedFramerates':[]},{'allowedFramerates':[1]},{'maxResolution':5},{'maxFramerate':5},{'bitrateMatrixOverrides':[1]},
                 {'bitrateMatrixOverrides':{'x':1}},{'bitrateMatrixOverrides':{'720_30':0}},{'minBitrateKbps':30000},
                 {'maxBitrateKbps':9000,'minBitrateKbps':600,'bitrateStepKbps':100,'allowedResolutions':['native',1080,720],'allowedFramerates':[60,30],'maxResolution':1440,'maxFramerate':90,'allowCustomBitrate':False,'bitrateMatrixOverrides':{'720_30':2500}},
                 {'bitrateMatrixOverrides':None,'discoveryEnabled':False}]:
        c.call('root','PATCH','/api/settings/streaming',body)
    c.call('al','GET','/api/settings/streaming')
    for body in [{'instanceName':''},{'instanceName':'x'*33},{'federatedRegistrationOpen':1},{'directoryBrowseEnabled':'x'},{'supportCardEnabled':0},
                 {'maxUploadSizeMb':0},{'maxUploadSizeMb':'12'},{'federationRelayTtlDays':0},{'defaultAutoRotateIntervalDays':400},
                 {'directoryEnabled':'x'},{'directoryEnabled':True},
                 {'instanceName':' Мой Сервер ','registrationOpen':False,'federatedRegistrationOpen':False,'directoryBrowseEnabled':False,'supportCardEnabled':False,'gifApiKey':' abcdef123456 ','federationRelayTtlDays':7,'defaultAutoRotateIntervalDays':30,'autoAcceptPeering':False},
                 {'gifApiKey':'****3456','discoveryEnabled':True},{'directoryEnabled':True},{'gifApiKey':'','registrationOpen':True}]:
        c.call('root','PATCH','/api/settings/instance',body)
    c.call(None,'GET','/api/instance/info',label='info')
if __name__=='__main__':
    bad=run(admin, show='-v' in sys.argv)+run(settings, show='-v' in sys.argv); sys.exit(1 if bad else 0)
