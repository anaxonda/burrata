#include <webkit2/webkit2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int forwarded, destroyed;
static void generic_handler(WebKitUserContentManager *manager, WebKitJavascriptResult *value, gpointer data) {
  (void)manager; (void)value;
  if (data == GINT_TO_POINTER(0x51)) forwarded++;
}
static void handler_destroyed(gpointer data, GClosure *closure) {
  (void)closure;
  if (data == GINT_TO_POINTER(0x51)) destroyed++;
}

int main(void) {
  WebKitUserContentManager *manager = webkit_user_content_manager_new();
  g_signal_connect_data(manager, "script-message-received", G_CALLBACK(generic_handler),
    GINT_TO_POINTER(0x51), handler_destroyed, 0);
  if (!webkit_user_content_manager_register_script_message_handler(manager, "ipc")) return 10;
  guint signal_id = g_signal_lookup("script-message-received", WEBKIT_TYPE_USER_CONTENT_MANAGER);
  g_signal_emit(manager, signal_id, g_quark_from_static_string("ipc"), NULL);
  g_signal_emit(manager, signal_id, g_quark_from_static_string("ipc"), NULL);
  if (forwarded != 2) return 11;
  g_object_unref(manager);
  if (destroyed != 1) return 12;
  const char *path = getenv("BURRATA_IPC_COUNTER_PATH");
  FILE *file = path ? fopen(path, "r") : NULL;
  if (!file) return 13;
  char json[128] = {0};
  size_t size = fread(json, 1, sizeof json - 1, file);
  fclose(file);
  unsigned long registered = 0, count = 0;
  if (!size || sscanf(json, "{\"registered\": %lu,\"count\": %lu}", &registered, &count) != 2)
    return 14;
  return registered == 1 && count == 2 ? 0 : 15;
}
