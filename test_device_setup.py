"""KUAL orchestration against temporary worker/OTA/reboot stubs only."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE=Path(__file__).resolve().parent


class SetupFlow(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name);self.root=self.base/'root';self.package=self.base/'package';self.commands=self.base/'commands';self.out=self.base/'out';self.inbox=self.base/'inbox';self.calls=self.base/'calls';self.failure=self.base/'failure'
        for p in [self.root/'bin',self.root/'install',self.package/'bin',self.package/'install',self.package/'ota',self.commands,self.inbox]:p.mkdir(parents=True,exist_ok=True)
        self.env={**os.environ,'PATH':str(self.commands)+':/usr/bin:/bin'}
        self.tool('id','echo 0\n');self.tool('lipc-get-prop','case "$2" in userstoreIsAvailable) echo 1;; cmState) echo CONNECTED;;esac\n')
        for name in ('sha256sum','sync'):self.tool(name,'exit 0\n')
        self.tool('reboot','echo reboot >> '+str(self.calls)+'\n')
        # Actual sha tool is required by installed-vs-candidate readback.
        self.tool('sha256sum','if [ "$1" = -c ];then exit 0;fi\nshasum -a 256 "$@"\n')
        (self.package/'SHA256SUMS').write_text('')
        worker='''#!/bin/sh
echo "$3" >> CALLS
if [ -f FAILURE ] && [ "$(cat FAILURE)" = "$3" ];then exit 17;fi
if [ "$3" = import-config ];then : > ROOT/config.json;: > ROOT/auth.json;fi
printf '{"state":"completed","network_writes":false}\n'
'''.replace('CALLS',str(self.calls)).replace('FAILURE',str(self.failure)).replace('ROOT',str(self.root))
        for name in ['ksync-worker','ksync-collector','ksync-event-adapter']:
            p=self.package/'bin'/name;p.write_text(worker);p.chmod(0o700);shutil.copyfile(p,self.root/'bin'/name);(self.root/'bin'/name).chmod(0o700)
        preview=self.package/'install/preview-upgrade.sh';preview.write_text('#!/bin/sh\necho preview >> '+str(self.calls)+'\n: > '+str(self.root/'new-binaries')+'\n')
        control=self.root/'install/control.sh';control.write_text('#!/bin/sh\necho "$1" >> '+str(self.calls)+'\n')
        ota=self.package/'ota/control.sh';ota.write_text('#!/bin/sh\necho "$1" >> '+str(self.calls)+'\nif [ -f '+str(self.failure)+' ] && [ "$(cat '+str(self.failure)+')" = "$1" ];then exit 19;fi\n')
        body=(HERE/'device/kual/bootstrap.sh').read_text().replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin','PATH='+str(self.commands)+':/usr/bin:/bin').replace('PACKAGE=/mnt/us/kindle-native-sync-package','PACKAGE='+str(self.package)).replace('ROOT=/var/local/kindle-native-sync','ROOT='+str(self.root)).replace('OUT=/mnt/us/.kindle-native-sync-export','OUT='+str(self.out)).replace('/mnt/us/.kindle-native-sync-inbox',str(self.inbox)).replace('/mnt/us/.kindle-native-sync-actions',str(self.base/'actions'))
        self.script=self.base/'bootstrap.sh';self.script.write_text(body)

    def tool(self,name,body):
        p=self.commands/name;p.write_text('#!/bin/sh\n'+body);p.chmod(0o700)

    def run_action(self,action):return subprocess.run(['sh',str(self.script),action],env=self.env,capture_output=True,text=True,timeout=5)

    def steps(self):return self.calls.read_text().splitlines() if self.calls.exists() else []

    def test_setup_commits_preview_before_import_and_reboots_only_after_guard(self):
        r=self.run_action('setup-preview');self.assertEqual(r.returncode,0,r.stderr)
        self.assertEqual(self.steps(),['preview','self-test','import-config','guard','enable','reboot'])
        self.assertEqual(json.loads((self.out/'setup-result.json').read_text())['state'],'setup_complete_restart_requested')

    def test_post_import_guard_failure_keeps_configuration_without_reboot_or_rollback(self):
        self.failure.write_text('guard')
        r=self.run_action('setup-preview');self.assertEqual(r.returncode,19,r.stderr)
        self.assertEqual(self.steps(),['preview','self-test','import-config','guard'])
        self.assertTrue((self.root/'new-binaries').exists());self.assertTrue((self.root/'config.json').exists());self.assertTrue((self.root/'auth.json').exists())
        result=json.loads((self.out/'setup-result.json').read_text());self.assertEqual(result['stage'],'protect_firmware');self.assertFalse(result['account_time_write_performed'])

    def test_local_selftest_failure_does_not_import_or_reboot(self):
        self.failure.write_text('self-test')
        r=self.run_action('setup-preview');self.assertEqual(r.returncode,17,r.stderr)
        self.assertEqual(self.steps(),['preview','self-test']);self.assertFalse((self.root/'config.json').exists())

    def test_resume_does_not_try_unconfigured_upgrade_again(self):
        (self.root/'config.json').write_text('{}');(self.root/'auth.json').write_text('{}');self.inbox.rmdir()
        r=self.run_action('resume-setup');self.assertEqual(r.returncode,0,r.stderr)
        self.assertEqual(self.steps(),['self-test','guard','enable','reboot'])

    def test_prepare_checks_ready_and_tls_before_authorization(self):
        r=self.run_action('authorize-prepare');self.assertEqual(r.returncode,0,r.stderr)
        self.assertEqual(self.steps(),['require-ready','self-test','operate'])
        self.assertTrue((self.out/'action-authorize-prepare.json').exists())
        self.calls.write_text('');self.failure.write_text('require-ready')
        r=self.run_action('authorize-prepare');self.assertEqual(r.returncode,19)
        self.assertEqual(self.steps(),['require-ready'])

    def test_confirmation_is_one_worker_action_without_shell_enable_or_retry(self):
        self.failure.write_text('operate')
        r=self.run_action('confirm-enable');self.assertEqual(r.returncode,17)
        self.assertEqual(self.steps(),['operate'])
        result=json.loads((self.out/'action-confirm-enable.json').read_text());self.assertEqual(result['state'],'operation_failed');self.assertFalse(result['replay_allowed'])


if __name__=='__main__':unittest.main()
