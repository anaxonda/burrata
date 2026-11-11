"""Exact private AX controls. No coordinates, generic action indices, or backend Play."""
import argparse
import json
from pathlib import Path
import sys
import time
import private_actor_base as base
import radio_ui_actor as radio

BUTTONS={'button','push button','toggle button'}

def ancestors(node,nodes):
    by_path={n['path']:n for n in nodes};parent=by_path.get(node['parent']);out=[]
    while parent and len(out)<48:
        out.append(parent);parent=by_path.get(parent['parent'])
    return out

def tagged(node,tag):
    try:
        attrs=node['object'].get_attributes()
        return isinstance(attrs,dict) and attrs.get('toolkit')=='WebKitGTK' and attrs.get('tag')==tag
    except Exception:return False

def choose_track(nodes,title,scope,occurrence=1,expected_rows=1):
    targets=[n for n in nodes if n['name']=='Play '+title and n['role'] in BUTTONS
      and any(a['name']==scope for a in ancestors(n,nodes))]
    if not 1<=len(targets)<=400:raise ValueError('UI_TRACK_CONTROL_AMBIGUOUS_OR_MISSING')
    rows=[next((a['path'] for a in ancestors(n,nodes) if tagged(a,'li')),None) for n in targets]
    if None in rows or len(set(rows))!=expected_rows or not 1<=occurrence<=expected_rows:raise ValueError('UI_TRACK_ROW_AMBIGUOUS')
    if any(rows.count(path)>2 for path in set(rows)):raise ValueError('UI_TRACK_ROW_TOO_MANY_PLAY_CONTROLS')
    # AX child paths encode actual DOM order; numeric components avoid /10 < /2.
    ordered=sorted(set(rows),key=lambda path:tuple(int(i) for i in path.split('/')[1:]))
    targets=[target for target,row in zip(targets,rows) if row==ordered[occurrence-1]]
    # TrackList intentionally exposes both position-play and track-title buttons
    # with the same label and handler. Either is valid only inside one exact li.
    return next((n for n in targets if radio.visible(n)),targets[0])

def choose_queue(nodes,title,index,expected_rows=None):
    scopes=[n for n in nodes if n['name']=='Playback queue']
    if len(scopes)!=1:raise ValueError('UI_QUEUE_SCOPE_AMBIGUOUS')
    queue_path=scopes[0]['path']
    articles=[n for n in nodes if tagged(n,'article') and n['path'].startswith(queue_path+'/')]
    if expected_rows is not None and len(articles)!=expected_rows:raise ValueError('UI_QUEUE_ROW_COUNT_MISMATCH')
    anchors=[n for n in nodes if n['name']==f'Select queue occurrence {index}: {title}' and n['role'] in BUTTONS]
    if len(anchors)!=1:raise ValueError('UI_QUEUE_OCCURRENCE_ANCHOR_AMBIGUOUS')
    rows=[a for a in ancestors(anchors[0],nodes) if tagged(a,'article')]
    if not rows or not any(a['name']=='Playback queue' for a in ancestors(rows[0],nodes)):
        raise ValueError('UI_QUEUE_OCCURRENCE_NOT_ATTESTED')
    targets=[n for n in nodes if n['name']=='Play '+title and n['role'] in BUTTONS and n['path'].startswith(rows[0]['path']+'/')]
    if len(targets)!=1:raise ValueError('UI_QUEUE_PLAY_AMBIGUOUS')
    return targets[0]

def valid_queue_occurrence_request(title,index,expected_rows):
    return isinstance(title,str) and bool(title) and type(index) is int and type(expected_rows) is int and 1<=index<=expected_rows<=200

def activate(node,atspi):
    obj=node['object'];obj.clear_cache_single()
    state=obj.get_state_set()
    if not state.contains(atspi.StateType.ENABLED):raise ValueError('UI_TARGET_DISABLED')
    if not radio.visible({**node,'bounds':radio.rect(obj,atspi),'showing':state.contains(atspi.StateType.SHOWING)}):
        if not atspi.Component.scroll_to(obj.get_component_iface(),atspi.ScrollType.ANYWHERE):raise ValueError('UI_TARGET_NOT_VISIBLE')
        base.pump_events();obj.clear_cache_single();state=obj.get_state_set()
        if not radio.visible({**node,'bounds':radio.rect(obj,atspi),'showing':state.contains(atspi.StateType.SHOWING)}):raise ValueError('UI_TARGET_NOT_REVEALED')
    if base.name_of(obj)!=node['name']:raise ValueError('UI_TARGET_CHANGED')
    names=radio.action_names(obj,atspi)
    actions=[i for i,name in enumerate(names) if name in {'click','press','activate','toggle'}]
    if len(actions)!=1:raise ValueError('UI_NAMED_ACTION_AMBIGUOUS')
    target_ready=time.monotonic_ns()
    activation_started=time.monotonic_ns()
    accepted=atspi.Action.do_action(obj.get_action_iface(),actions[0])
    activation_returned=time.monotonic_ns()
    if not accepted:raise ValueError('UI_ACTION_REJECTED')
    return {'passed':True,'activated':True,'action':names[actions[0]],'target':node['name'],'path':node['path'],
      'bounds':radio.rect(obj,atspi),'method':'named-private-accessibility-action','playbackProven':False,
      'targetReadyMonotonicNs':target_ready,
      'activationStartedMonotonicNs':activation_started,'activationReturnedMonotonicNs':activation_returned,
      'activationCallDurationNs':activation_returned-activation_started}

def queue_selected_action(nodes,root,title,scope,atspi):
    starters=[n for n in nodes if n['name']=='Select tracks' and n['role'] in BUTTONS]
    if len(starters)!=1:raise ValueError('UI_QUEUE_SELECTED_START_AMBIGUOUS')
    started=activate(starters[0],atspi)
    base.pump_events();nodes=radio.collect(root,atspi,{'Play '+title,'Select '+title,'Queue selected'})
    radio.refresh_targets(nodes,{'Play '+title,'Select '+title,'Queue selected'},atspi)
    play=choose_track(nodes,title,scope,1,1)
    row=next((a for a in ancestors(play,nodes) if tagged(a,'li')),None)
    checks=[n for n in nodes if n['name']=='Select '+title and n['role'] in {'check box','checkbox'}
      and row is not None and n['path'].startswith(row['path']+'/')]
    if len(checks)!=1:raise ValueError('UI_QUEUE_SELECTED_CHECKBOX_AMBIGUOUS')
    chosen=activate(checks[0],atspi)
    base.pump_events();nodes=radio.collect(root,atspi,{'Queue selected'})
    radio.refresh_targets(nodes,{'Queue selected'},atspi)
    targets=[n for n in nodes if n['name']=='Queue selected' and n['role'] in BUTTONS]
    if len(targets)!=1:raise ValueError('UI_QUEUE_SELECTED_BUTTON_AMBIGUOUS')
    result=activate(targets[0],atspi)
    result['selectionActions']=[started,chosen]
    return result

def perform_action(args,nodes,root,atspi):
    if args.mode=='favorite':
        area=json.loads(args.area or '{}')
        if not all(type(area.get(k)) in (int,float) for k in ['x','y','width','height']):raise ValueError('UI_FOOTER_AREA_INVALID')
        return radio.favorite(nodes,root,area,atspi)
    if args.mode=='queue-selected':return queue_selected_action(nodes,root,args.title,args.scope,atspi)
    if args.mode=='track':node=choose_track(nodes,args.title,args.scope,args.occurrence,args.expected_rows)
    elif args.mode=='queue-occurrence':node=choose_queue(nodes,args.title,args.index,args.expected_queue_rows)
    elif args.mode=='album-play':
        targets=[n for n in nodes if n['name']=='Play album '+args.title and n['role'] in BUTTONS]
        if len(targets)!=1:raise ValueError('UI_ALBUM_PLAY_AMBIGUOUS')
        node=targets[0]
    else:
        targets=[n for n in nodes if n['name']=='Play all 3 saved tracks in playlist order, ignoring filters' and n['role'] in BUTTONS]
        if len(targets)!=1:raise ValueError('UI_PLAYLIST_BUTTON_AMBIGUOUS')
        node=targets[0]
    return activate(node,atspi)

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--profile',type=Path,required=True);p.add_argument('--pid',type=int,required=True)
    p.add_argument('--dbus',required=True);p.add_argument('--xdg',type=Path,required=True)
    p.add_argument('--mode',choices=['track','queue-occurrence','queue-selected','album-play','playlist-play','favorite'],required=True)
    p.add_argument('--title',default='');p.add_argument('--scope',choices=['Recordings','Artist recordings','Spotify results','Discogs results'])
    p.add_argument('--index',type=int);p.add_argument('--expected-queue-rows',type=int);p.add_argument('--area')
    p.add_argument('--occurrence',type=int,default=1);p.add_argument('--expected-rows',type=int,default=1)
    args=p.parse_args()
    try:
        base.checked_context(args.profile,args.pid,args.dbus,args.xdg)
        if len(args.title)>300 or any(ord(c)<32 for c in args.title):raise ValueError('UI_TITLE_INVALID')
        if args.mode=='track' and (not args.title or not args.scope):raise ValueError('UI_TRACK_CONTEXT_INVALID')
        if args.mode=='album-play' and not args.title:raise ValueError('UI_ALBUM_CONTEXT_INVALID')
        if args.mode=='queue-selected' and (not args.title or not args.scope):raise ValueError('UI_QUEUE_SELECTED_CONTEXT_INVALID')
        if not 1<=args.occurrence<=args.expected_rows<=200:raise ValueError('UI_TRACK_OCCURRENCE_INVALID')
        if args.mode=='queue-occurrence' and not valid_queue_occurrence_request(args.title,args.index,args.expected_queue_rows):
            raise ValueError('UI_QUEUE_CONTEXT_INVALID')
        import gi
        gi.require_version('Atspi','2.0')
        from gi.repository import Atspi
        Atspi.init();root=base.app_accessible(args.pid,Atspi)
        names={'Playback queue','Play '+args.title,'Select queue occurrence '+str(args.index)+': '+args.title,
          'Play album '+args.title,'Select '+args.title,'Select tracks','Queue selected',
          'Play all 3 saved tracks in playlist order, ignoring filters','Add playing track to favorites','Remove playing track from favorites'}
        base.pump_events();nodes=radio.collect(root,Atspi,names);radio.refresh_targets(nodes,names,Atspi)
        result=perform_action(args,nodes,root,Atspi)
        print(json.dumps({'status':'private_ui_action','mode':args.mode,**result}));return 0
    except Exception as error:
        code=str(error) if str(error).startswith(('UI_','PRIVATE_','RADIO_')) else 'UI_ACTOR_FAILED'
        print(json.dumps({'status':'rejected','code':code}),file=sys.stderr);return 2

if __name__=='__main__':raise SystemExit(main())
