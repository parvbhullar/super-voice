/* Sofia-SIP bindgen wrapper header.
 * nua.h transitively includes su_wait.h (for su_root_t, su_root_create,
 * su_root_destroy, su_root_step) and sip.h. No separate su_root.h exists
 * in the installed headers — su_root functions live in su_wait.h.
 */
#include <sofia-sip/nua.h>
#include <sofia-sip/sip.h>
#include <sofia-sip/sip_util.h>
#include <sofia-sip/sdp.h>
#include <sofia-sip/auth_module.h>
