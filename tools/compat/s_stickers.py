import sys, base64; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
from s_files import png
def stickers(c):
    a=c.user('al'); b=c.user('bo')
    img=base64.b64encode(png(600,300)).decode()
    small=base64.b64encode(png(20,10)).decode()
    c.call('al','GET','/api/stickers')
    c.call(None,'GET','/api/stickers')
    for body in [None, [], {}, {'name':'x'}, {'image':img}, {'name':'','image':small}, {'name':'я'*101,'image':small},
                 {'name':['x'],'image':small}, {'name':5,'image':small}, {'name':None,'image':small}, {'name':'x','image':'abc'},
                 {'name':'x','image':{}}, {'name':'   ','image':small}, {'name':'a\nb','image':small},
                 {'name':'x','image':'!!!!'}, {'name':'x','image':small[:-2]+'=='}, {'name':'x','image':'AAAA'},
                 {'name':'x','image':base64.b64encode(b'<svg xmlns="http://www.w3.org/2000/svg"/>').decode()}]:
        c.call('al','POST','/api/stickers',body)
    st=c.call('al','POST','/api/stickers',{'name':'  Широкий  ','image':img,'extra':1})
    c.call('al','POST','/api/stickers',{'name':'другое имя','image':img})
    st2=c.call('al','POST','/api/stickers',{'name':5,'image':small})
    c.call('al','GET','/api/stickers')
    sid=st['id']; tok=st['token']
    s,h,body=c.raw(None,'GET',f'/api/stickers/assets/{sid}.webp',keep=('content-type','cache-control','content-security-policy','x-content-type-options','x-frame-options'),auth=False,label='картинка стикера')
    c.log[-1]=(c.log[-1][0], c.log[-1][1], {'headers': c.log[-1][2]['headers'], 'webp': bytes(body[:4])==b'RIFF' and bytes(body[8:12])==b'WEBP'})
    for path in [f'/api/stickers/assets/{sid}', f'/api/stickers/assets/{sid}.png', '/api/stickers/assets/'+'0'*64+'.webp', '/api/stickers/assets/x.webp', f'/api/stickers/assets/{sid}.webp.webp']:
        c.call(None,'GET',path)
    # чужая коллекция
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{})
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{'token':tok})
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{'token':'sticker:https://other.example/api/stickers/assets/'+sid+'.webp','name':'x'})
    c.call('bo','POST','/api/stickers/nope/collect',{'token':tok,'name':'x'})
    c.call('bo','POST','/api/stickers/'+'0'*64+'/collect',{'token':tok.replace(sid,'0'*64),'name':'x'})
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{'token':tok,'name':'  '})
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{'token':tok,'name':' Мой '})
    c.call('bo','POST',f'/api/stickers/{sid}/collect',{'token':tok,'name':'Повтор'})
    c.call('bo','GET','/api/stickers')
    c.call('bo','PATCH',f'/api/stickers/{sid}',{})
    c.call('bo','PATCH',f'/api/stickers/{sid}',{'name':'​'})
    c.call('bo','PATCH','/api/stickers/nope',{'name':'x'})
    c.call('bo','PATCH','/api/stickers/'+'1'*64,{'name':'x'})
    c.call('bo','PATCH',f'/api/stickers/{sid}',{'name':' Новое '})
    c.call('bo','DELETE','/api/stickers/nope')
    c.call('bo','DELETE',f'/api/stickers/{sid}')
    c.call('bo','DELETE',f'/api/stickers/{sid}')
    c.call('bo','GET','/api/stickers')
    # стикер в переписке: без встраивания и не находится поиском
    did=c.call('al','POST','/api/dm',{'userId':b})['id']
    c.call('al','POST',f'/api/dm/{did}/messages',{'content':tok}); c.settle(1.2)
    c.call('al','POST',f'/api/dm/{did}/messages',{'content':'про sticker словом'}); c.settle()
    c.send('bo',{'type':'reaction_add','messageId':st['id'],'emoji':tok}); c.settle()
    c.call('bo','GET',f'/api/dm/{did}/messages')
    c.call('bo','GET',f'/api/dm/{did}/search?q=sticker')

if __name__=='__main__': sys.exit(1 if run(stickers, show='-v' in sys.argv) else 0)
