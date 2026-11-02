"""Exact production shell permission repair against temporary directories only."""
from pathlib import Path
import os
import stat
import subprocess
import sys
import tempfile
import unittest


class PrivateRootRepair(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name);self.root=self.base/'private';self.root.mkdir(mode=0o755)
        self.commands=self.base/'commands';self.commands.mkdir()
        self.env={**os.environ,'PATH':str(self.commands)+':/usr/bin:/bin'}
        tool=self.commands/'id';tool.write_text('#!/bin/sh\nprintf "%s\\n" "${TEST_EFFECTIVE_UID:-0}"\n');tool.chmod(0o700)
        tool=self.commands/'stat';tool.write_text('#!'+sys.executable+'\nimport os,stat,sys\nfrom pathlib import Path\nif os.environ.get("TEST_STAT_FAILURE"):raise SystemExit(1)\nif sys.argv[2]=="%u":print(os.environ.get("TEST_OWNER_UID","0"))\nelse:print(format(stat.S_IMODE(Path(sys.argv[3]).lstat().st_mode),"o"))\n');tool.chmod(0o700)
        self.common=self.base/'common.sh'
        self.common.write_text(Path(__file__).with_name('common.sh').read_text().replace('ROOT=/var/local/kindle-native-sync','ROOT='+str(self.root)))

    def run_repair(self,**env):
        return subprocess.run(['sh','-c','. "$1"; repair_private_root','repair',str(self.common)],env={**self.env,**env},capture_output=True,text=True,timeout=3)

    def test_only_owned_real_root_mode_changes(self):
        child=self.root/'auth-fixture';child.write_text('fixture');child.chmod(0o644)
        result=self.run_repair();self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(stat.S_IMODE(self.root.stat().st_mode),0o700)
        self.assertEqual(stat.S_IMODE(child.stat().st_mode),0o644)

    def test_other_uid_or_nonroot_or_stat_failure_refuses_without_chmod(self):
        for env in [{'TEST_OWNER_UID':'501'},{'TEST_EFFECTIVE_UID':'501'},{'TEST_STAT_FAILURE':'1'}]:
            with self.subTest(env=env):
                self.root.chmod(0o755)
                self.assertNotEqual(self.run_repair(**env).returncode,0)
                self.assertEqual(stat.S_IMODE(self.root.stat().st_mode),0o755)

    def test_symlink_missing_or_file_is_never_repaired(self):
        target=self.base/'target';self.root.rename(target);self.root.symlink_to(target,target_is_directory=True)
        self.assertNotEqual(self.run_repair().returncode,0)
        self.assertEqual(stat.S_IMODE(target.stat().st_mode),0o755)
        self.root.unlink();self.assertNotEqual(self.run_repair().returncode,0)
        self.assertFalse(self.root.exists())
        self.root.write_text('fixture');self.root.chmod(0o644)
        self.assertNotEqual(self.run_repair().returncode,0)
        self.assertEqual(stat.S_IMODE(self.root.stat().st_mode),0o644)


if __name__=='__main__':unittest.main()
