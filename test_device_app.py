"""Synthetic accounts, temporary disks and local HTTP only; no real device/API."""
import hashlib
from contextlib import closing
import http.client
import json
from pathlib import Path
import shutil
import sqlite3
import tempfile
import threading
import unittest
from unittest.mock import patch

import device_app
import device_bridge as bridge

HERE=Path(__file__).resolve().parent


class DeviceAppTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name);self.base=self.root/'device-project';self.base.mkdir()
        (self.base/'config-web').mkdir();shutil.copyfile(HERE/'config-web/index.html',self.base/'config-web/index.html')
        self.mount=self.root/'kindle';(self.mount/'documents').mkdir(parents=True)
        (self.mount/'system').mkdir();(self.mount/'system/version.txt').write_text('Kindle 5.12.2.1.1')
        export=self.mount/'.kindle-native-sync-export';export.mkdir()
        (export/'probe.json').write_text(json.dumps({'root':True,'supported':True,'firmware':'5.12.2.1.1'}))
        self.app=device_app.DeviceApp(self.base,mount=self.mount,known_legacy_projects=[]);self.addCleanup(self.app.close)
        self.auth={'vid':'synthetic-account','deviceId':'kindle-only','accessToken':'SYNTHETIC_ACCESS','refreshToken':'SYNTHETIC_REFRESH'}
        self.write(self.base/'.weread/device-auth.json',self.auth)

    def write(self,path,value):
        path.parent.mkdir(parents=True,exist_ok=True);path.write_text(json.dumps(value));path.chmod(0o600)

    def legacy(self):
        old=self.root/'actual-old';private=old/'.weread';private.mkdir(parents=True)
        self.write(private/'auth.json',{**self.auth,'deviceId':'mac-only'})
        with closing(sqlite3.connect(private/'sync-engine.sqlite')) as db,db:
            db.executescript('CREATE TABLE plans(state TEXT);CREATE TABLE attempts(batch_id TEXT,state TEXT,seconds INTEGER);CREATE TABLE mappings(account TEXT,source_key TEXT,weread_id TEXT,title TEXT);')
            db.execute("INSERT INTO attempts VALUES ('unresolved-old','needs_review',455)")
            db.execute('INSERT INTO mappings VALUES (?,?,?,?)',(hashlib.sha256(self.auth['vid'].encode()).hexdigest(),'A','123','Synthetic Book'))
        return old

    def choose_new(self):
        self.app.dispatch('/api/source',{'mode':'new','no_existing_uploader':True})

    def test_fresh_is_explicit_and_no_initial_empty_ledger(self):
        self.assertFalse((self.base/'.weread/sync-engine.sqlite').exists())
        with self.assertRaises(ValueError):self.app.dispatch('/api/source',{'mode':'new'})
        with self.assertRaises(ValueError):self.app.dispatch('/api/device/handoff',{'confirmed':True})
        self.choose_new();one=self.app.dispatch('/api/device/handoff',{'confirmed':True})
        self.assertEqual(one['old_pending_count'],0);self.assertEqual(one['kind'],'device-upload-fence')
        result=self.app.dispatch('/api/device/config',{'confirmed':True})
        self.assertFalse(result['auto_upload_enabled'])
        config=json.loads((self.mount/bridge.INBOX/'config.json').read_text())
        self.assertEqual(config['mappings'],[]);self.assertFalse(config['validation']['device_validation']);self.assertFalse(config['validation']['protocol_validation'])
        self.assertNotIn('SYNTHETIC_ACCESS',json.dumps(self.app.state()))

    def test_detected_legacy_cannot_be_bypassed_with_fresh_mode(self):
        old=self.legacy();self.app.known_legacy_projects=[old]
        with self.assertRaises(ValueError):self.choose_new()
        self.assertFalse((self.base/'.weread/sync-engine.sqlite').exists())

    def test_empty_clone_cannot_be_selected_as_legacy(self):
        empty=self.root/'empty';empty.mkdir()
        with self.assertRaises(ValueError):self.app.dispatch('/api/source',{'mode':'legacy','legacy_project':str(empty)})
        self.assertFalse((empty/'.weread').exists())

    def test_wrong_account_stops_before_fencing_legacy(self):
        old=self.legacy();self.app.dispatch('/api/source',{'mode':'legacy','legacy_project':str(old)})
        self.write(self.base/'.weread/device-auth.json',{**self.auth,'vid':'different-account'})
        with self.assertRaises(ValueError):self.app.dispatch('/api/device/handoff',{'confirmed':True})
        self.assertFalse((old/'.weread'/bridge.FENCE).exists())
        with closing(sqlite3.connect(old/'.weread/sync-engine.sqlite')) as db:self.assertEqual(db.execute('SELECT state,seconds FROM attempts').fetchone(),('needs_review',455))

    def test_legacy_handoff_carries_only_explicit_mappings_and_preserves_uncertain(self):
        old=self.legacy();self.app.dispatch('/api/source',{'mode':'legacy','legacy_project':str(old)})
        before=(old/'.weread/auth.json').read_bytes()
        fence=self.app.dispatch('/api/device/handoff',{'confirmed':True})
        self.assertEqual(fence['old_pending_count'],1)
        rows=self.app.dispatch('/api/mappings/legacy',{})['mappings']
        self.assertEqual(rows,[{'local_book_id':'A','weread_book_id':'123','title':'Synthetic Book'}])
        self.app.dispatch('/api/device/config',{'confirmed':True})
        config=json.loads((self.mount/bridge.INBOX/'config.json').read_text())
        self.assertEqual(config['mappings'],rows);self.assertNotIn('455',json.dumps(config))
        with closing(sqlite3.connect(old/'.weread/sync-engine.sqlite')) as db:self.assertEqual(db.execute('SELECT state,seconds FROM attempts').fetchone(),('needs_review',455))
        self.assertEqual((old/'.weread/auth.json').read_bytes(),before)
        with self.assertRaises(ValueError):self.choose_new()

    def test_same_legacy_cli_argument_restarts_after_handoff_but_switch_is_blocked(self):
        old=self.legacy();self.app.dispatch('/api/source',{'mode':'legacy','legacy_project':str(old)})
        self.app.dispatch('/api/device/handoff',{'confirmed':True})
        resumed=device_app.DeviceApp(self.base,legacy_project=old,mount=self.mount,known_legacy_projects=[])
        self.addCleanup(resumed.close)
        self.assertEqual(resumed.settings['legacy_project'],str(old.resolve()))
        self.assertTrue(resumed.state()['mac_upload_fenced'])
        other=self.root/'other';shutil.copytree(old,other)
        with self.assertRaises(ValueError):device_app.DeviceApp(self.base,legacy_project=other,mount=self.mount,known_legacy_projects=[])

    def test_search_is_readonly_nested_results_plus_personal_shelf(self):
        class Fake:
            calls=[]
            def request(self,method,url,**kwargs):
                self.calls.append((method,url))
                if '/store/search' in url:return {'results':[{'bookInfo':{'bookId':'123','title':'Synthetic Book','author':'A','accessToken':'DO_NOT_RETURN'}}]}
                return {'books':[{'bookId':'CB_personal','title':'Synthetic Book personal','author':'A'}]}
        with patch.object(device_app,'Transport',Fake):result=self.app.dispatch('/api/search',{'keyword':'Synthetic'})
        self.assertEqual({x['bookId'] for x in result['books']},{'123','CB_personal'})
        self.assertTrue(all(method=='GET' for method,_ in Fake.calls));self.assertNotIn('DO_NOT_RETURN',json.dumps(result))

    def test_mapping_validation_does_not_accept_guessed_blank_ids(self):
        for rows in [None,[{'local_book_id':'A','weread_book_id':''}],[{'local_book_id':'A','weread_book_id':'1'},{'local_book_id':'A','weread_book_id':'2'}]]:
            with self.assertRaises(ValueError):self.app.dispatch('/api/mappings/save',{'mappings':rows})
        self.assertFalse((self.base/'.weread/mappings.json').exists())

    def test_candidate_inventory_uses_metadata_and_keeps_unparsed_files_unbound(self):
        docs = self.mount/'documents'
        book = docs/'Author'/'Exact File.mobi'
        book.parent.mkdir()
        book.write_bytes(b'book contents are not read')
        self.write(self.mount/'metadata.calibre', [
            {'lpath':'documents/Author/Exact File.mobi','title':'Candidate Title','authors':['Author']}
        ])
        (docs/'link.mobi').symlink_to(book)
        hidden = docs/'secret.sdr'
        hidden.mkdir()
        (hidden/'not-a-book.mobi').write_bytes(b'private sidecar')
        result = self.app.book_candidates()
        self.assertEqual(len(result['books']), 1)
        candidate = result['books'][0]
        self.assertEqual(candidate['title'], 'Candidate Title')
        self.assertIsNone(candidate['local_book_id'])
        self.assertEqual(candidate['candidate_id'], 'documents/Author/Exact File.mobi')

    def test_candidate_save_requires_confirmation_and_preserves_active_mappings(self):
        (self.mount/'documents'/'Candidate.mobi').write_bytes(b'not parsed')
        original = [{'local_book_id':'A','weread_book_id':'123','title':'Existing'}]
        self.app.mappings(original)
        request = {'candidate_id':'documents/Candidate.mobi',
                   'target':{'bookId':'456','title':'Candidate Target','author':'Author'}}
        with self.assertRaises(ValueError):
            self.app.dispatch('/api/books/save', request)
        result = self.app.dispatch('/api/books/save', {**request,'confirmed':True})
        self.assertTrue(result['awaiting_native_identity'])
        self.assertFalse(result['device_updated'])
        self.assertEqual(self.app.state()['mappings'], original)
        self.app.dispatch('/api/books/save', {**request,'confirmed':True})
        saved = json.loads((self.base/'.weread/book-candidates.json').read_text())
        self.assertEqual(len(saved), 1)
        (self.mount/'documents'/'Candidate.mobi').unlink()
        with self.assertRaises(ValueError):
            self.app.dispatch('/api/books/save', {**request,'confirmed':True})

    def test_http_guards_and_no_import_or_time_upload_routes(self):
        server=device_app.ThreadingHTTPServer(('127.0.0.1',0),device_app.make_handler(self.app));port=server.server_address[1]
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        self.addCleanup(server.server_close);self.addCleanup(server.shutdown)
        connection=http.client.HTTPConnection('127.0.0.1',port,timeout=3);self.addCleanup(connection.close)
        connection.request('GET','/');response=connection.getresponse();self.assertEqual(response.status,200);self.assertIn('Kindle'.encode(),response.read())
        connection.request('GET','/.weread/device-auth.json');response=connection.getresponse();self.assertEqual(response.status,404);self.assertNotIn(b'SYNTHETIC_ACCESS',response.read())
        connection.request('POST','/api/source',body=json.dumps({'mode':'new','no_existing_uploader':True}),headers={'Content-Type':'application/json'})
        response=connection.getresponse();self.assertEqual(response.status,403);response.read()
        headers={'Content-Type':'application/json','Origin':f'http://127.0.0.1:{port}','X-CSRF-Token':self.app.csrf}
        connection.request('POST','/api/source',body=json.dumps({'mode':'new','no_existing_uploader':True}),headers=headers)
        response=connection.getresponse();self.assertEqual(response.status,200);response.read()
        for path in ['/api/import','/api/commit','/api/sync']:
            connection.request('POST',path,body='{}',headers=headers);response=connection.getresponse();self.assertEqual(response.status,400);response.read()
        connection.request('GET','/api/device',headers={'Host':'evil.example'});response=connection.getresponse();self.assertEqual(response.status,403);response.read()


if __name__=='__main__':unittest.main()
