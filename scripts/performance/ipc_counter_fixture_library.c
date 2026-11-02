#include <glib-object.h>

/* Stand-in for WebKit's exported registration entry point. */
gboolean webkit_user_content_manager_register_script_message_handler(gpointer manager, const gchar *name) {
  if (g_strcmp0(g_getenv("IPC_COUNTER_FIXTURE_FAIL_IPC"), "1") == 0 && g_strcmp0(name, "ipc") == 0)
    return FALSE;
  GQuark key = g_quark_from_static_string("fixture-registered-channel");
  GHashTable *channels = g_object_get_qdata(G_OBJECT(manager), key);
  if (!channels) {
    channels = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, NULL);
    g_object_set_qdata_full(G_OBJECT(manager), key, channels, (GDestroyNotify)g_hash_table_unref);
  }
  if (g_hash_table_contains(channels, name)) return FALSE;
  g_hash_table_add(channels, g_strdup(name));
  return TRUE;
}
