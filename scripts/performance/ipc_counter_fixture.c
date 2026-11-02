#include <glib-object.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct { GObject parent; } FixtureManager;
typedef struct { GObjectClass parent; } FixtureManagerClass;
G_DEFINE_TYPE(FixtureManager, fixture_manager, G_TYPE_OBJECT)

static guint signal_id;
static int forwarded, destroyed;

static void fixture_manager_class_init(FixtureManagerClass *klass) {
  signal_id = g_signal_new("script-message-received", G_TYPE_FROM_CLASS(klass),
    G_SIGNAL_RUN_LAST | G_SIGNAL_DETAILED, 0, NULL, NULL, NULL, G_TYPE_NONE, 1, G_TYPE_POINTER);
}
static void fixture_manager_init(FixtureManager *self) { (void)self; }
static void generic_handler(FixtureManager *manager, gpointer value, gpointer data) {
  (void)manager; (void)value;
  if (data == GINT_TO_POINTER(0x51)) forwarded++;
}
static void handler_destroyed(gpointer data, GClosure *closure) {
  (void)closure;
  if (data == GINT_TO_POINTER(0x51)) destroyed++;
}
extern gboolean webkit_user_content_manager_register_script_message_handler(gpointer, const gchar *);

static int check_counter(unsigned long want_registered, unsigned long want_count) {
  const char *path = g_getenv("BURRATA_IPC_COUNTER_PATH");
  FILE *file = path ? fopen(path, "r") : NULL;
  if (!file) return want_registered == 0 && want_count == 0;
  char json[128] = {0};
  size_t size = fread(json, 1, sizeof json - 1, file);
  fclose(file);
  unsigned long actual_registered = 999, actual_count = 999;
  return size > 0 && sscanf(json, "{\"registered\": %lu,\"count\": %lu}",
    &actual_registered, &actual_count) == 2 &&
    actual_registered == want_registered && actual_count == want_count;
}

static FixtureManager *new_manager(void) {
  FixtureManager *manager = g_object_new(fixture_manager_get_type(), NULL);
  g_signal_connect_data(manager, "script-message-received", G_CALLBACK(generic_handler),
    GINT_TO_POINTER(0x51), handler_destroyed, 0);
  return manager;
}

int main(int argc, char **argv) {
  const char *mode = argc > 1 ? argv[1] : "ipc";
  FixtureManager *manager = new_manager();
  if (strcmp(mode, "non-ipc") == 0) {
    if (!webkit_user_content_manager_register_script_message_handler(manager, "other")) return 10;
    g_signal_emit(manager, signal_id, g_quark_from_string("other"), (gpointer)0x3);
    if (forwarded != 1 || !check_counter(0, 0)) return 11;
  } else if (strcmp(mode, "failed-ipc") == 0) {
    if (webkit_user_content_manager_register_script_message_handler(manager, "ipc")) return 12;
    /* A synthetic emission proves a failed registration did not attach an observer. */
    g_signal_emit(manager, signal_id, g_quark_from_string("ipc"), (gpointer)0x4);
    if (forwarded != 1 || !check_counter(0, 0)) return 13;
  } else {
    if (!webkit_user_content_manager_register_script_message_handler(manager, "ipc")) return 14;
    if (webkit_user_content_manager_register_script_message_handler(manager, "ipc")) return 15;
    if (!webkit_user_content_manager_register_script_message_handler(manager, "other")) return 16;
    g_signal_emit(manager, signal_id, g_quark_from_string("ipc"), (gpointer)0x1);
    g_signal_emit(manager, signal_id, g_quark_from_string("ipc"), (gpointer)0x2);
    g_signal_emit(manager, signal_id, g_quark_from_string("other"), (gpointer)0x3);
    if (forwarded != 3 || !check_counter(1, 2)) return 17;
  }
  g_object_unref(manager);
  if (destroyed != 1) return 18;
  return 0;
}
