#!/usr/bin/env python3
"""Bounded app-owned radio AX checks and one exact favorite-button activation."""
import argparse
import json
from pathlib import Path
import sys
import time
import private_actor_base as base

LINK_ACTIONS={'click','press','activate','open','jump'}

def link_attestation(obj):
    try:
        role=obj.get_role_name().strip().lower();wire=int(obj.get_role());attrs=obj.get_attributes()
        native=role=='link'
        narrow=(wire==87 and role=='form' and isinstance(attrs,dict) and attrs.get('toolkit')=='WebKitGTK'
          and attrs.get('tag')=='a' and attrs.get('computed-role')=='link')
        return {'accepted':native or narrow,'method':'native-link' if native else 'wire87-webkit-anchor' if narrow else 'rejected',
          'wireRole':wire,'rawRole':role if role in {'link','form'} else 'other'}
    except Exception:return {'accepted':False,'method':'rejected','wireRole':None,'rawRole':'other'}

def action_names(obj,atspi):
    try:
        iface=obj.get_action_iface()
        return [str(atspi.Action.get_action_name(iface,i)).strip().lower() for i in range(min(8,int(atspi.Action.get_n_actions(iface))))]
    except Exception:return []

def rect(obj,atspi):
    try:
        r=obj.get_component_iface().get_extents(atspi.CoordType.SCREEN)
        return {key:int(getattr(r,key)) for key in ['x','y','width','height']}
    except Exception:return None

def collect(root,atspi,targets=None):
    queue=[(root,'app',None,0)];nodes=[];visited=0
    while queue and visited<12000:
        obj,path,parent,depth=queue.pop(0);visited+=1
        try:
            name=str(obj.get_name() or '').strip()
            wanted=targets is None or name in targets
            state=obj.get_state_set() if wanted else None
            node={'object':obj,'path':path,'parent':parent,'name':name,
              'role':str(obj.get_role_name() or '').strip().lower() if wanted else '',
              'bounds':rect(obj,atspi) if wanted else None,
              'showing':bool(state and state.contains(atspi.StateType.SHOWING)),
              'enabled':bool(state and state.contains(atspi.StateType.ENABLED)),
              'pressed':bool(state and state.contains(atspi.StateType.PRESSED)),
              'link':link_attestation(obj) if wanted and name.endswith('↗') else {'accepted':False}}
            nodes.append(node)
            if depth<48:
                for i in range(min(max(0,int(obj.get_child_count())),12000-visited)):
                    child=obj.get_child_at_index(i)
                    if child is not None:queue.append((child,f'{path}/{i}',path,depth+1))
        except Exception:continue
    if queue:raise ValueError('RADIO_AX_TREE_LIMIT')
    return nodes

def refresh_targets(nodes,names,atspi):
    # Keep default bulk cache for traversal. Only exact proof/action candidates
    # receive a fresh state/bounds/name read; never invalidate the whole tree.
    candidates=[n for n in nodes if n['name'] in names]
    if len(candidates)>24:raise ValueError('RADIO_AX_TARGET_LIMIT')
    for node in candidates:
        obj=node['object']
        try:
            obj.clear_cache_single();state=obj.get_state_set()
            node.update(name=str(obj.get_name() or '').strip(),bounds=rect(obj,atspi),
              showing=bool(state.contains(atspi.StateType.SHOWING)),enabled=bool(state.contains(atspi.StateType.ENABLED)),
              pressed=bool(state.contains(atspi.StateType.PRESSED)))
        except Exception:raise ValueError('RADIO_AX_TARGET_REFRESH_FAILED') from None
    return nodes

def visible(node):
    r=node.get('bounds') or {}
    return node.get('showing') is True and r.get('width',0)>0 and r.get('height',0)>0

def within(node,area):
    r=node.get('bounds') or {}
    return visible(node) and r['x']>=area['x']-1 and r['y']>=area['y']-1 and r['x']+r['width']<=area['x']+area['width']+1 and r['y']+r['height']<=area['y']+area['height']+1

def safe(node):
    return {k:node.get(k) for k in ['name','role','path','bounds','showing','enabled','pressed','link']}

def source_links(nodes,name,atspi,require_source_row=False):
    links=[n for n in nodes if n['name']==name and n['link']['accepted'] and visible(n)]
    if len(links)!=1:raise ValueError('RADIO_SOURCE_LINK_NOT_UNIQUE_VISIBLE')
    if not links[0]['enabled']:raise ValueError('RADIO_SOURCE_LINK_DISABLED')
    link=links[0];actions=action_names(link['object'],atspi)
    if not any(action in LINK_ACTIONS for action in actions):raise ValueError('RADIO_SOURCE_LINK_ACTION_MISSING')
    row_labels=[]
    if require_source_row:
        by_path={n['path']:n for n in nodes};parent=by_path.get(link['parent']);grand=by_path.get(parent['parent']) if parent else None
        row_path=grand['path'] if grand else None
        row_labels=[n for n in nodes if n['name']=='Source' and visible(n) and row_path and n['path'].startswith(row_path+'/')]
        if not row_labels:raise ValueError('RADIO_INFO_SOURCE_ROW_NOT_ATTESTED')
    return {'passed':True,'link':safe(link),'sourceLabelCount':len(row_labels),'actions':[a if a in LINK_ACTIONS else 'other' for a in actions],
      'clicked':False,'externalUrlReadOrOpened':False}

def context_check(nodes,station,area):
    labels=[n for n in nodes if n['name']=='PLAYING FROM' and within(n,area)]
    contexts=[n for n in nodes if n['name']==station and within(n,area)]
    if not labels or not contexts:raise ValueError('RADIO_FULLSCREEN_CONTEXT_NOT_VISIBLE')
    return {'passed':True,'playingFromCount':len(labels),'stationCount':len(contexts),'station':station,'withinExpandedPlayer':True}

def favorite(nodes,root,area,atspi):
    matches=[n for n in nodes if n['name']=='Add playing track to favorites' and n['role'] in {'button','push button','toggle button'} and within(n,area) and n['enabled']]
    if len(matches)!=1:raise ValueError('RADIO_FAVORITE_BUTTON_NOT_UNIQUE')
    node=matches[0];actions=action_names(node['object'],atspi)
    index=next((i for i,a in enumerate(actions) if a in {'click','press','activate','toggle'}),None)
    if index is None:raise ValueError('RADIO_FAVORITE_ACTION_MISSING')
    if not atspi.Action.do_action(node['object'].get_action_iface(),index):raise ValueError('RADIO_FAVORITE_ACTION_REJECTED')
    end=time.monotonic()+3
    while time.monotonic()<end:
        base.pump_events();after=collect(root,atspi,{'Remove playing track from favorites'})
        refresh_targets(after,{'Remove playing track from favorites'},atspi)
        changed=[n for n in after if n['name']=='Remove playing track from favorites' and n['role'] in {'button','push button','toggle button'} and within(n,area) and n['enabled']]
        if len(changed)==1:return {'passed':True,'clicked':True,'before':safe(node),'after':safe(changed[0]),'action':'named-accessibility-action'}
        time.sleep(.1)
    raise ValueError('RADIO_FAVORITE_POSTCONDITION_MISSING')

def target_diagnostics(nodes,station,area,atspi):
    targets={'source-label':'Source','station-link':'Station website ↗','info-link':station+' ↗',
      'playing-from':'PLAYING FROM','station-context':station,'expanded-region':'Expanded Now Playing',
      'close-fullscreen':'Close full-screen Now Playing'}
    allowed_names=set(targets.values());by_path={n['path']:n for n in nodes}
    roles={'form','link','static','text','label','paragraph','section','panel','heading','unknown','button','push button','toggle button'}
    tags={'a','dt','dd','dl','div','section','header','small','strong','span','p','button'}
    def compact(n):
        obj=n['object'];attrs={}
        try:attrs=obj.get_attributes()
        except Exception:pass
        try:role=str(obj.get_role_name() or '').strip().lower()
        except Exception:role='other'
        try:showing=bool(obj.get_state_set().contains(atspi.StateType.SHOWING))
        except Exception:showing=False
        if not isinstance(attrs,dict):attrs={}
        return {'path':n['path'],'parent':n['parent'],'nameClass':'exact-target' if n['name'] in allowed_names else 'empty' if not n['name'] else 'other',
          'role':role if role in roles else 'other','bounds':rect(obj,atspi),'showing':showing,
          'toolkit':'WebKitGTK' if attrs.get('toolkit')=='WebKitGTK' else 'other',
          'tag':attrs.get('tag') if attrs.get('tag') in tags else 'other',
          'linkAttestation':n.get('link',{'accepted':False}),
          'actions':[a if a in LINK_ACTIONS else 'other' for a in action_names(obj,atspi)] if n.get('link',{}).get('accepted') and n['name'] in allowed_names else []}
    counts=[]
    for label,name in targets.items():
        exact=[n for n in nodes if n['name']==name]
        counts.append({'target':label,'count':len(exact),'showing':sum(visible(n) for n in exact),
          'withinArea':sum(within(n,area) for n in exact) if area else 0,'nodes':[compact(n) for n in exact[:3]]})
    links=[n for n in nodes if n['name']==station+' ↗' and n['link']['accepted']]
    ancestors=[];neighbors=[]
    if len(links)==1:
        parent=by_path.get(links[0]['parent'])
        for _ in range(4):
            if not parent:break
            ancestors.append(compact(parent));parent=by_path.get(parent['parent'])
        if len(ancestors)>1:
            row=ancestors[1]['path'];near=[n for n in nodes if n['path'].startswith(row+'/') and n['path'].count('/')-row.count('/')<=3]
            neighbors=[compact(n) for n in near[:8]]
    return {'visitedNodes':len(nodes),'targetCounts':counts,'sourceAncestors':ancestors,'sourceNeighbors':neighbors}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile',type=Path,required=True);parser.add_argument('--pid',type=int,required=True)
    parser.add_argument('--dbus',required=True);parser.add_argument('--xdg',type=Path,required=True)
    parser.add_argument('--mode',choices=['station-link','info-source','fullscreen-context','favorite'],required=True)
    parser.add_argument('--station',required=True);parser.add_argument('--area')
    args=parser.parse_args()
    nodes=[];area=None;Atspi=None
    try:
        base.checked_context(args.profile,args.pid,args.dbus,args.xdg)
        area=json.loads(args.area) if args.area else None
        if args.mode in {'favorite','fullscreen-context'} and (not isinstance(area,dict) or not all(type(area.get(k)) in (int,float) for k in ['x','y','width','height']) or area['width']<=0 or area['height']<=0):
            raise ValueError('RADIO_AX_AREA_INVALID')
        import gi
        gi.require_version('Atspi','2.0')
        from gi.repository import Atspi
        Atspi.init();root=base.app_accessible(args.pid,Atspi)
        targets={'Add playing track to favorites','Remove playing track from favorites'} if args.mode=='favorite' else {
          'PLAYING FROM',args.station,'Expanded Now Playing','Close full-screen Now Playing'} if args.mode=='fullscreen-context' else {
          'Station website ↗'} if args.mode=='station-link' else {'Source',args.station+' ↗'}
        base.pump_events();nodes=collect(root,Atspi,targets);refresh_targets(nodes,targets,Atspi)
        if args.mode=='favorite':result=favorite(nodes,root,area,Atspi)
        elif args.mode=='fullscreen-context':result=context_check(nodes,args.station,area)
        else:
            target='Station website ↗' if args.mode=='station-link' else args.station+' ↗'
            candidates=[n for n in nodes if n['name']==target and n['link']['accepted']]
            revealed=False
            if len(candidates)==1 and not visible(candidates[0]):
                revealed=bool(Atspi.Component.scroll_to(candidates[0]['object'].get_component_iface(),Atspi.ScrollType.ANYWHERE))
                base.pump_events();nodes=collect(root,Atspi,targets);refresh_targets(nodes,targets,Atspi)
            result=source_links(nodes,target,Atspi,args.mode=='info-source')
            result['scrolledToExactLink']=revealed
        print(json.dumps({'status':'private_radio_ax','mode':args.mode,**result}));return 0
    except Exception as error:
        reason=str(error)
        if not reason.startswith(('RADIO_','PRIVATE_')):reason='RADIO_AX_FAILED'
        diagnostic=None
        if nodes and Atspi is not None:
            try:diagnostic=target_diagnostics(nodes,args.station,area,Atspi)
            except Exception:diagnostic={'visitedNodes':len(nodes),'targetCounts':[]}
        print(json.dumps({'status':'rejected','mode':args.mode,'reasonCode':reason,'errorType':type(error).__name__,
          'diagnostics':diagnostic}),file=sys.stderr);return 2
if __name__=='__main__':raise SystemExit(main())
