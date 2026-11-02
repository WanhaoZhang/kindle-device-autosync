#!/usr/bin/env python3
"""Loopback-only device configuration. No Mac reading-log importer or time uploader."""
import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import threading
import time
import urllib.parse
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import device_bridge as bridge
from weread_readonly import BridgeError, Transport, load_auth


class DeviceApp:
    def __init__(self, base, *, legacy_project=None, mount='/Volumes/Kindle', known_legacy_projects=None):
        self.base=Path(base).resolve()
        self.private=bridge._directory(self.base/'.weread')
        self.mount=Path(mount)
        self.csrf=secrets.token_urlsafe(32)
        self.lock=threading.Lock()
        self.known_legacy_projects=list(known_legacy_projects) if known_legacy_projects is not None else [Path.home()/'Documents/KindleReadingStats']
        self.settings_path=self.private/'assistant-state.json'
        self.settings=bridge._read(self.settings_path) if self.settings_path.exists() else {'mode':'unselected'}
        self.login_process=None;self.login_output=None;self.qr_path=None;self.login_started=None
        if legacy_project is not None:self.select_source({'mode':'legacy','legacy_project':str(legacy_project)})

    def source(self):
        return Path(self.settings['legacy_project']) if self.settings.get('mode')=='legacy' else self.base

    def select_source(self, value):
        if (value.get('mode')=='legacy' and self.settings.get('mode')=='legacy'
                and isinstance(value.get('legacy_project'),str)
                and Path(value['legacy_project']).expanduser().resolve()==Path(self.settings['legacy_project'])):
            return {'state':'selected',**self.settings}
        if (self.private/bridge.FENCE).exists() or bridge.status(self.source(),self.mount)['mac_upload_fenced']:
            raise ValueError('已有交接记录，不能切换来源或建立另一空实例。')
        mode=value.get('mode')
        if mode=='legacy':
            raw=value.get('legacy_project')
            if not isinstance(raw,str) or not raw.strip():raise ValueError('请填写真实旧项目目录。')
            project=Path(raw).expanduser().resolve();private=project/'.weread'
            if project==self.base or not (private/'sync-engine.sqlite').is_file() or not (private/'auth.json').is_file():
                raise ValueError('旧项目须包含已有账号凭据及 sync-engine.sqlite，不能选择空克隆。')
            if private.is_symlink() or (private/'sync-engine.sqlite').is_symlink() or (private/'auth.json').is_symlink():
                raise ValueError('旧项目私有数据不能是符号链接。')
            self.settings={'mode':'legacy','legacy_project':str(project)}
        elif mode=='new':
            if value.get('no_existing_uploader') is not True:raise ValueError('请明确确认没有其他上传实例。')
            for project in self.known_legacy_projects:
                private=Path(project)/'.weread'
                if (private/'sync-engine.sqlite').is_file() and (private/'auth.json').is_file():
                    raise ValueError('检测到已有网页版实例，请选择它完成交接。')
            self.settings={'mode':'new','no_existing_uploader':True}
        else:raise ValueError('请选择迁移旧项目或明确的新安装。')
        bridge._write(self.settings_path,self.settings)
        return {'state':'selected',**self.settings}

    def login_status(self):
        if self.login_process is None:return {'state':'idle','purpose':'kindle'}
        code=self.login_process.poll()
        if code is None:
            out={'state':'waiting','purpose':'kindle'}
            if self.qr_path and self.qr_path.is_file():out['qr_url']='/login-qr.png?v='+str(self.login_started)
            return out
        if code==0:return {'state':'success','purpose':'kindle'}
        return {'state':'error','purpose':'kindle','message':'扫码未完成，请重新生成；原凭据与账本保持保留。'}

    def state(self):
        out=bridge.status(self.source(),self.mount)
        auth=self.private/'device-auth.json'
        try:load_auth(auth);ready=True
        except BridgeError:ready=False
        out.update(device_auth_ready=ready,login=self.login_status(),source=self.settings,
                   known_legacy_projects=[str(p) for p in self.known_legacy_projects if (Path(p)/'.weread/sync-engine.sqlite').is_file() and (Path(p)/'.weread/auth.json').is_file()])
        mapping=self.private/'mappings.json'
        out['mappings']=bridge._read(mapping) if mapping.exists() else []
        return out

    def start_login(self):
        if self.login_process and self.login_process.poll() is None:return self.login_status()
        if (self.private/'device-auth.json').exists():raise ValueError('已有 Kindle 专用凭据。不会在此隐藏切换设备身份；重新授权请先按维护说明核对账本。')
        if self.login_output:self.login_output.close()
        self.login_started=time.time_ns();self.qr_path=self.private/('login-'+str(self.login_started)+'.png')
        fd=os.open(self.private/'login-worker.log',os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600)
        self.login_output=os.fdopen(fd,'wb')
        self.login_process=subprocess.Popen([sys.executable,str(self.base/'weread_readonly.py'),'--credentials',str(self.private/'device-auth.json'),'login','--online','--qr-png',str(self.qr_path)],cwd=self.base,stdout=self.login_output,stderr=self.login_output)
        return self.login_status()

    def mappings(self, rows):
        if not isinstance(rows,list) or len(rows)>200:raise ValueError('对应关系须为最多200项的JSON数组。')
        result=[];seen=set()
        for row in rows:
            if not isinstance(row,dict):raise ValueError('书籍对应格式无效。')
            clean={}
            for key in ('local_book_id','weread_book_id','title'):
                value=row.get(key,'')
                if not isinstance(value,str) or len(value)>1024 or '\x00' in value:raise ValueError('书籍标识或标题无效。')
                if key!='title' and not value:raise ValueError('书籍标识不能为空。')
                clean[key]=value
            if clean['local_book_id'] in seen:raise ValueError('重复的本地书籍标识。')
            seen.add(clean['local_book_id']);result.append(clean)
        bridge._write(self.private/'mappings.json',result)
        return {'state':'saved','mappings_count':len(result)}

    def dispatch(self, path, body):
        with self.lock:
            if path=='/api/source':return self.select_source(body)
            if path=='/api/device/login':return self.start_login()
            if path=='/api/mappings/save':return self.mappings(body.get('mappings'))
            if path=='/api/mappings/legacy':
                if self.settings.get('mode')!='legacy':raise ValueError('请先选择旧项目。')
                rows=bridge.read_mappings(self.source(),self.private/'device-auth.json');self.mappings(rows)
                return {'mappings':rows,'mappings_count':len(rows)}
            if path=='/api/search':
                keyword=body.get('keyword')
                if not isinstance(keyword,str) or not 1<=len(keyword)<=100:raise ValueError('请输入书名或作者。')
                auth=load_auth(self.private/'device-auth.json')
                result=Transport().request('GET','https://i.weread.qq.com/store/search',query={'keyword':keyword,'scope':10,'maxIdx':0,'count':20},credentials=auth)
                # Read-only server responses stay local; select only book identity metadata.
                found={}
                def walk(item, depth=0, shelf=False):
                    if depth>12:return
                    if isinstance(item,list):
                        for child in item[:200]:walk(child,depth+1,shelf)
                    elif isinstance(item,dict):
                        if item.get('bookId') and isinstance(item.get('title'),str):
                            matches=keyword.casefold() in (item['title']+' '+str(item.get('author',''))).casefold()
                            if not shelf or matches:
                                row={k:str(item[k]) for k in ('bookId','title','author','publisher','isbn') if isinstance(item.get(k),(str,int))}
                                found[row['bookId']]=row
                        for child in item.values():
                            if isinstance(child,(dict,list)):walk(child,depth+1,shelf)
                walk(result)
                shelf_available=True
                try:walk(Transport().request('GET','https://i.weread.qq.com/shelf/sync',credentials=auth).get('books',[]),shelf=True)
                except BridgeError:shelf_available=False
                return {'books':list(found.values())[:40],'shelf_available':shelf_available}
            if path=='/api/device/handoff':
                if body.get('confirmed') is not True:raise ValueError('须明确确认交接。')
                device_auth=load_auth(self.private/'device-auth.json')
                probe=bridge.probe_usb(self.mount).get('root_probe') or {}
                if probe.get('root') is not True or probe.get('supported') is not True:raise ValueError('先从 Kindle 导出并核对兼容性探针。')
                if self.settings.get('mode')=='legacy':
                    legacy_auth=load_auth(self.source()/'.weread/auth.json')
                    if legacy_auth['vid']!=device_auth['vid'] or legacy_auth['deviceId']==device_auth['deviceId']:
                        raise ValueError('须以同一账号建立独立 Kindle 设备身份；交接尚未写入。')
                    return bridge.prepare_handoff(self.source(),confirmed=True,preserve_pending=True)
                if self.settings.get('mode')=='new':
                    return bridge.initialize_new_install(self.base,confirmed=True,no_existing_uploader=True,known_legacy_projects=self.known_legacy_projects)
                raise ValueError('尚未选择迁移或新安装。')
            if path=='/api/device/config':
                if body.get('confirmed') is not True:raise ValueError('须明确确认 USB 暂存凭据。')
                path=self.private/'mappings.json';rows=bridge._read(path) if path.exists() else []
                return bridge.export_config(self.source(),self.mount,credentials_path=self.private/'device-auth.json',mappings=rows)
        raise ValueError('不支持的操作。')

    def close(self):
        if self.login_process and self.login_process.poll() is None:self.login_process.terminate()
        if self.login_output:self.login_output.close()


def make_handler(app):
    class Handler(BaseHTTPRequestHandler):
        server_version='KindleDeviceConfig/1'
        def log_message(self,*_):pass
        def valid_host(self):
            port=self.server.server_address[1]
            return self.headers.get('Host') in (f'127.0.0.1:{port}',f'localhost:{port}')
        def reply(self,code,value,kind='application/json; charset=utf-8'):
            raw=value if isinstance(value,bytes) else json.dumps(value,ensure_ascii=False).encode()
            self.send_response(code);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(raw)));self.send_header('Cache-Control','no-store');self.send_header('X-Content-Type-Options','nosniff');self.end_headers();self.wfile.write(raw)
        def do_GET(self):
            if not self.valid_host():return self.reply(403,{'ok':False,'error':'Host不受支持。'})
            path=urllib.parse.urlsplit(self.path).path
            if path=='/api/health':return self.reply(200,{'app':'kindle-device-config','version':1})
            if path=='/api/bootstrap':return self.reply(200,{'csrf_token':app.csrf})
            if path in ('/','/device'):return self.reply(200,(app.base/'config-web/index.html').read_bytes(),'text/html; charset=utf-8')
            if path=='/login-qr.png' and app.qr_path and app.qr_path.is_file():return self.reply(200,app.qr_path.read_bytes(),'image/png')
            if path=='/api/device':
                try:return self.reply(200,{'ok':True,'data':app.state()})
                except Exception:return self.reply(500,{'ok':False,'error':'本地状态不可读取，请保留文件。'})
            return self.reply(404,{'ok':False,'error':'没有此页面。'})
        def do_POST(self):
            port=self.server.server_address[1]
            if not self.valid_host() or self.headers.get('Origin') not in (f'http://127.0.0.1:{port}',f'http://localhost:{port}') or not secrets.compare_digest(self.headers.get('X-CSRF-Token',''),app.csrf):
                return self.reply(403,{'ok':False,'error':'请求来源或校验码无效。'})
            try:
                size=int(self.headers.get('Content-Length','0'))
                if not 0<size<=256*1024 or self.headers.get('Content-Type','').split(';')[0]!='application/json':raise ValueError('请求格式或大小不受支持。')
                body=json.loads(self.rfile.read(size))
                if not isinstance(body,dict):raise ValueError('请求须为JSON对象。')
                data=app.dispatch(urllib.parse.urlsplit(self.path).path,body)
                return self.reply(200,{'ok':True,'data':data})
            except (ValueError,BridgeError) as exc:return self.reply(400,{'ok':False,'error':str(exc)})
            except Exception:return self.reply(500,{'ok':False,'error':'操作未完成；没有打印凭据，请保留原文件。'})
    return Handler


def main(argv=None):
    parser=argparse.ArgumentParser(description='Kindle独立配置助手：扫码、旧实例交接、映射、USB配置。没有Mac时长上传器。')
    parser.add_argument('--port',type=int,default=8766);parser.add_argument('--legacy-project',type=Path)
    parser.add_argument('--mount',type=Path,default=Path('/Volumes/Kindle'));parser.add_argument('--open-browser',action='store_true')
    args=parser.parse_args(argv)
    app=DeviceApp(Path(__file__).resolve().parent,legacy_project=args.legacy_project,mount=args.mount)
    try:server=ThreadingHTTPServer(('127.0.0.1',args.port),make_handler(app))
    except OSError:raise SystemExit('本地端口不可用，请关闭已有助手或指定--port。')
    port=server.server_address[1]
    print(f'Kindle配置助手：http://127.0.0.1:{port}',flush=True)
    if args.open_browser:webbrowser.open(f'http://127.0.0.1:{port}')
    try:server.serve_forever()
    except KeyboardInterrupt:pass
    finally:app.close();server.server_close()


if __name__=='__main__':main()
