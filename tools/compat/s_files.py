import sys, base64, io, struct, zlib; sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
from scn import run
b64=lambda s: base64.b64encode(s.encode()).decode()
T={'Tus-Resumable':'1.0.0'}
OCT='application/offset+octet-stream'
K=('upload-offset','upload-length','upload-defer-length','upload-expires','cache-control','location','tus-resumable')
def png(w,h):
    """Простейший PNG без сжатия содержимого по смыслу: градиент."""
    raw=b''.join(b'\x00'+bytes((x*7+y*3)%256 for x in range(w) for _ in range(3)) for y in range(h))
    chunk=lambda t,d: struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
    return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',w,h,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
def upload(c, who, name, data, keep=K):
    s,h,_=c.raw(who,'POST','/api/files',{**T,'Upload-Length':str(len(data)),'Upload-Metadata':'filename '+b64(name)},keep=keep)
    loc=h['location']
    s,h,b=c.raw(who,'PATCH',loc,{**T,'Upload-Offset':'0','Content-Type':OCT},data,keep=keep+('content-type',))
    return b
def files(c):
    a=c.user('al'); b=c.user('bo')
    c.raw(None,'POST','/api/files',{**T,'Upload-Length':'5'},auth=False,keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Authorization':'Bearer junk'},auth=False,keep=K)
    c.raw('al','POST','/api/files',{'Upload-Length':'5'},keep=K)
    c.raw('al','POST','/api/files',{**T},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5'},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Metadata':'filename'},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Metadata':'flag,bad !!!'},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Metadata':'filename '+b64('   ')},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'999999999999','Upload-Metadata':'filename '+b64('a.txt')},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'abc','Upload-Metadata':'filename '+b64('a.txt')},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'-1','Upload-Metadata':'filename '+b64('a')},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Defer-Length':'1','Upload-Metadata':'filename '+b64('a')},keep=K)
    c.raw('al','POST','/api/files',{**T,'Upload-Defer-Length':'2','Upload-Metadata':'filename '+b64('a')},keep=K)
    c.raw('al','POST','/api/files',{'Tus-Resumable':'0.2.0','Upload-Length':'5','Upload-Metadata':'filename '+b64('a')},keep=K)
    s,h,_=c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Metadata':'filename '+b64('при вет.TXT')+',filetype '+b64('text/plain')+',flag'},keep=K)
    loc=h['location']
    s,h,_=c.raw('al','HEAD',loc,T,keep=K)
    meta=[(p.split(' ')[0], base64.b64decode(p.split(' ')[1]).decode() if ' ' in p else None) for p in h.get('upload-metadata','').split(',')]
    c.log.append(('метаданные', 200, c.n.val([list(m) for m in meta])))
    c.raw('bo','HEAD',loc,T,keep=K+('content-length',))
    c.raw('al','HEAD','/api/files/'+'0'*32,T,keep=K+('content-length',))
    c.raw('al','HEAD','/api/files',T,keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'0'},b'he',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'0','Content-Type':'text/plain'},b'he',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'x','Content-Type':OCT},b'he',keep=K)
    c.raw('al','PATCH',loc,{**T,'Content-Type':OCT},b'he',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'3','Content-Type':OCT},b'he',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'0','Content-Type':OCT},b'',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'0','Content-Type':OCT},b'he',keep=K)
    c.raw('al','HEAD',loc,T,keep=K)
    c.raw('bo','PATCH',loc,{**T,'Upload-Offset':'2','Content-Type':OCT},b'llo',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'2','Content-Type':OCT},b'llo!!',keep=K)
    c.raw('al','HEAD',loc,T,keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'2','Upload-Length':'9','Content-Type':OCT},b'',keep=K)
    s,h,att=c.raw('al','PATCH',loc,{**T,'Upload-Offset':'2','Content-Type':OCT},b'llo',keep=K+('content-type',))
    c.raw('al','HEAD',loc,T,keep=K)
    c.raw('al','DELETE',loc,T,keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'5','Content-Type':OCT},b'',keep=K)
    c.raw('al','PUT',loc,T,keep=K)
    # отложенная длина
    s,h,_=c.raw('al','POST','/api/files',{**T,'Upload-Defer-Length':'1','Upload-Metadata':'filename '+b64('d.bin')},keep=K)
    loc=h['location']
    c.raw('al','HEAD',loc,T,keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'0','Content-Type':OCT},b'a',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'1','Upload-Length':'0','Content-Type':OCT},b'',keep=K)
    c.raw('al','PATCH',loc,{**T,'Upload-Offset':'1','Upload-Length':'2','Content-Type':OCT},b'b',keep=K+('content-type',))
    # отмена
    s,h,_=c.raw('al','POST','/api/files',{**T,'Upload-Length':'5','Upload-Metadata':'originalName '+b64('../../x.tar.GZ')+',filename '+b64('y.png')},keep=K)
    loc=h['location']
    c.raw('bo','DELETE',loc,T,keep=K)
    c.raw('al','DELETE',loc,T,keep=K)
    c.raw('al','DELETE',loc,T,keep=K)
    # раздача
    SK=('content-type','content-length','content-range','accept-ranges','cache-control','content-disposition','content-security-policy','x-content-type-options','x-frame-options')
    name=att['filename']
    c.raw(None,'GET',f'/api/uploads/{name}',keep=SK,auth=False)
    c.raw(None,'GET',f'/api/uploads/{name}',{'Range':'bytes=1-3'},keep=SK,auth=False)
    c.raw(None,'GET',f'/api/uploads/{name}',{'Range':'bytes=2-'},keep=SK,auth=False)
    c.raw(None,'GET',f'/api/uploads/{name}',{'Range':'bytes=0-0'},keep=SK,auth=False)
    c.raw(None,'GET','/api/uploads/nope.txt',auth=False)
    c.raw(None,'GET','/api/uploads/..%2F..%2Fbs.db',auth=False)
    # /api/uploads/.tus не сравнивается: оригинал отвечает 500 (каталог), здесь 404.
    # картинки: широкая получает миниатюру, узкая нет
    wide=upload(c,'al','Широкая.PNG',png(1000,40))
    c.raw(None,'GET',f"/api/uploads/{wide['filename']}",keep=SK,auth=False)
    s,h,body=c.raw(None,'GET',f"/api/uploads/{wide.get('thumbnailFilename','nope')}",keep=('content-type','cache-control','content-disposition'),auth=False,label='миниатюра')
    c.log[-1]=(c.log[-1][0], c.log[-1][1], {'headers': c.log[-1][2]['headers'], 'webp': bytes(body[:4])==b'RIFF' and bytes(body[8:12])==b'WEBP'})
    upload(c,'al','small.png',png(20,10))
    upload(c,'al','page.svg',b'<svg xmlns="http://www.w3.org/2000/svg"/>')
    svg=upload(c,'al','doc.pdf',b'%PDF-1.4')
    c.raw(None,'GET',f"/api/uploads/{svg['filename']}",keep=SK,auth=False)
    upload(c,'al','broken.jpg',b'not a jpeg')
    upload(c,'al','noext',b'x')
    upload(c,'al','.hidden',b'x')
    # вложение в сообщении
    did=c.call('al','POST','/api/dm',{'userId':b})['id']
    c.call('bo','POST',f'/api/dm/{did}/messages',{'content':'чужой файл','attachments':[wide['id']]})
    c.call('al','POST',f'/api/dm/{did}/messages',{'content':'файл','attachments':[wide['id'],att['id']]}); c.settle()
    c.call('al','POST',f'/api/dm/{did}/messages',{'content':'повтор','attachments':[wide['id']]})
    c.call('bo','GET',f'/api/dm/{did}/messages')
    c.call('bo','GET',f'/api/dm/{did}/search?has=image')
    c.call('bo','GET',f'/api/dm/{did}/search?has=file')

if __name__=='__main__': sys.exit(1 if run(files, show='-v' in sys.argv) else 0)
