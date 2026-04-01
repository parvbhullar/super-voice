/* crates/pjsip-sys/pjsip_wrapper.h
 * Aggregate header for pjproject B2BUA bindings.
 * Only includes headers needed for a SIP B2BUA — no audio device headers.
 */

/* pjlib base framework */
#include <pj/types.h>
#include <pj/pool.h>
#include <pj/log.h>
#include <pj/os.h>
#include <pj/string.h>
#include <pj/timer.h>
#include <pj/errno.h>

/* pjlib-util: DNS resolver for NAPTR/SRV (RFC 3263) */
#include <pjlib-util/resolver.h>
#include <pjlib-util/srv_resolver.h>
#include <pjlib-util/dns.h>

/* pjsip core: transport, transaction, message parsing */
#include <pjsip/sip_transport.h>
#include <pjsip/sip_transaction.h>
#include <pjsip/sip_endpoint.h>
#include <pjsip/sip_module.h>
#include <pjsip/sip_event.h>
#include <pjsip/sip_msg.h>
#include <pjsip/sip_uri.h>
#include <pjsip/sip_auth.h>
#include <pjsip/sip_dialog.h>
#include <pjsip/sip_ua_layer.h>
#include <pjsip/sip_util.h>
#include <pjsip/sip_resolve.h>

/* pjsip-ua: INVITE session (dialog + offer/answer) */
#include <pjsip-ua/sip_inv.h>
#include <pjsip-ua/sip_regc.h>
#include <pjsip-ua/sip_replaces.h>
#include <pjsip-ua/sip_xfer.h>
#include <pjsip-ua/sip_100rel.h>
#include <pjsip-ua/sip_timer.h>

/* pjsip-simple: presence / SUBSCRIBE / NOTIFY */
#include <pjsip-simple/evsub.h>

/* SDP (offer/answer; lives in pjmedia but has no audio device deps) */
#include <pjmedia/sdp.h>
#include <pjmedia/sdp_neg.h>
