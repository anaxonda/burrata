#!/usr/bin/env python3
"""Check source IPC permissions and exercise the delivered SQLite schema without Rust dependencies."""
import json
from pathlib import Path
import re
import sqlite3
import unittest

ROOT = Path(__file__).resolve().parent.parent

class Contracts(unittest.TestCase):
    def test_ipc_permissions_cover_registered_commands(self):
        core=(ROOT/'src-tauri/src/lib.rs').read_text()
        declared=set(re.findall(r'#\[tauri::command\]\s*(?:async\s+)?fn\s+(\w+)',core))
        handler=core.split('tauri::generate_handler![',1)[1].split('])',1)[0]
        registered=set(re.findall(r'\b[a-z][a-z0-9_]+\b',handler))
        build=set(re.findall(r'"([a-z0-9_]+)"',(ROOT/'src-tauri/build.rs').read_text().split('fn main',1)[0]))
        capability=json.loads((ROOT/'src-tauri/capabilities/main.json').read_text())
        allowed={p.removeprefix('allow-').replace('-','_') for p in capability['permissions'] if p.startswith('allow-')}
        self.assertEqual(declared,registered)
        self.assertEqual(registered,build)
        self.assertEqual(registered,allowed)
        self.assertEqual(capability['windows'],['main'])
        self.assertNotIn('remote',capability)
        ui=(ROOT/'src/App.svelte').read_text()
        invoked=set(re.findall(r'call(?:<[^>]+>)?\(\s*["\'](\w+)["\']',ui))
        self.assertTrue(invoked<=registered,invoked-registered)

    def test_no_remote_frames_and_required_icon(self):
        config=json.loads((ROOT/'src-tauri/tauri.conf.json').read_text())
        self.assertIn("frame-src 'none'",config['app']['security']['csp'])
        self.assertTrue(config['app']['withGlobalTauri'])
        self.assertTrue(any((ROOT/'src-tauri'/icon).is_file() for icon in config['bundle']['icon'] if icon.endswith('.png')))
        for source in (ROOT/'src-tauri/src').rglob('*.rs'):
            for include in re.findall(r'include_str!\(\s*"([^"]+)"',source.read_text()):
                self.assertTrue((source.parent/include).is_file(),f'Missing include source: {source}: {include}')

class Schema(unittest.TestCase):
    def setUp(self):
        self.db=sqlite3.connect(':memory:')
        self.db.execute('PRAGMA foreign_keys=ON')
        self.db.executescript((ROOT/'src-tauri/src/schema.sql').read_text())
    def tearDown(self):
        self.db.close()
    def add(self,identifier):
        self.db.execute('INSERT INTO works VALUES(?,?,?)',(identifier,'Night Drive','Artist'))
        self.db.execute('INSERT INTO recordings(id,work_id,version) VALUES(?,?,?)',(identifier,identifier,'Extended Mix'))
        self.db.execute('INSERT INTO catalog_listings VALUES(?,?,?,?,?,?,?)',(identifier,identifier.split(':')[0],identifier.split(':')[1],identifier,'https://example.invalid',json.dumps({'id':identifier}),'2026-10-11'))
    def test_schema_version_and_foreign_keys(self):
        self.assertEqual(self.db.execute('PRAGMA user_version').fetchone()[0],1)
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute('INSERT INTO recordings(id,work_id) VALUES(?,?)',('orphan','missing'))
    def test_listing_identity_and_asset_paths_are_unique(self):
        self.add('beatport:1')
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute('INSERT INTO catalog_listings VALUES(?,?,?,?,?,?,?)',('other','beatport','1','beatport:1','','{}','now'))
        self.db.execute('INSERT INTO local_assets VALUES(?,?,?,?,?)',('a','beatport:1','/music/a.flac','hash','{}'))
        with self.assertRaises(sqlite3.IntegrityError):
            self.db.execute('INSERT INTO local_assets VALUES(?,?,?,?,?)',('b','beatport:1','/music/a.flac','hash','{}'))
    def test_snapshot_rank_changes_do_not_rewrite_history(self):
        self.add('beatport:1')
        for identifier,rank in [('yesterday',9),('today',2)]:
            self.db.execute('INSERT INTO chart_snapshots VALUES(?,?,?,?,?)',(identifier,'beatport','chart',identifier,'{}'))
            self.db.execute('INSERT INTO chart_entries VALUES(?,?,?)',(identifier,'beatport:1',rank))
        self.assertEqual(self.db.execute('SELECT rank FROM chart_entries ORDER BY rank').fetchall(),[(2,),(9,)])
    def test_chart_memberships_use_latest_cached_rank(self):
        self.add('beatport:1')
        for day,rank in [('2026-10-10',9),('2026-10-11',2)]:
            self.db.execute('INSERT INTO chart_snapshots VALUES(?,?,?,?,?)',(day,'beatport','chart',day,json.dumps({'title':'House Top 100'})))
            self.db.execute('INSERT INTO chart_entries VALUES(?,?,?)',(day,'beatport:1',rank))
        storage=(ROOT/'src-tauri/src/storage.rs').read_text().split('pub fn chart_memberships',1)[1].split('pub fn latest_snapshot',1)[0]
        statement=re.search(r'prepare\(("(?:[^"\\]|\\.)*")',storage).group(1)
        row=self.db.execute(json.loads(statement),('beatport:1',)).fetchone()
        self.assertEqual(row,('beatport','House Top 100','chart',2,'2026-10-11'))

    def test_verified_merge_uses_real_store_sql_and_keeps_source_ranks(self):
        self.add('beatport:1');self.add('traxsource:2')
        self.db.execute("INSERT INTO collections VALUES('house','House')")
        for identifier in ['beatport:1','traxsource:2']:
            self.db.execute("INSERT INTO collection_members VALUES('house',?,1)",(identifier,))
        self.db.execute("INSERT INTO chart_snapshots VALUES('snapshot','traxsource','chart','now','{}')")
        self.db.execute("INSERT INTO chart_entries VALUES('snapshot','traxsource:2',3)")
        storage=(ROOT/'src-tauri/src/storage.rs').read_text()
        merge=storage.split('pub fn merge_recordings',1)[1].split('pub fn save_chart',1)[0]
        sqls=[json.loads(s) for s in re.findall(r'"(?:[^"\\]|\\.)*"',merge) if s.startswith(('"UPDATE','"INSERT','"DELETE'))]
        for sql in sqls:
            count=max((int(n) for n in re.findall(r'\?(\d+)',sql)),default=0)
            args=('traxsource:2',) if sql.startswith('DELETE') else ('beatport:1','traxsource:2')[:count]
            self.db.execute(sql,args)
        self.assertEqual(self.db.execute('SELECT COUNT(*) FROM collection_members').fetchone()[0],1)
        self.assertEqual(self.db.execute('SELECT rank FROM chart_entries').fetchone()[0],3)
        query=re.search(r'"collection"\s*=>\s*(?:\{\s*)?("(?:[^"\\]|\\.)*")',storage).group(1)
        rows=self.db.execute(json.loads(query),('house',)).fetchall()
        self.assertEqual(len(rows),1)
        self.assertEqual(json.loads(rows[0][0])['id'],'beatport:1')

if __name__=='__main__':
    unittest.main(verbosity=2)
