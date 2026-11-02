#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <glib-object.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

typedef gboolean (*register_fn)(gpointer, const gchar *);
static register_fn real_register;
static int counter_fd = -1;
static atomic_ulong registered = 0;
static atomic_ulong count = 0;
static pthread_mutex_t write_lock = PTHREAD_MUTEX_INITIALIZER;
static GQuark observer_key;

static void publish_counts(void) {
  if (counter_fd < 0) return;
  char json[128];
  int len = snprintf(json, sizeof json, "{\"registered\":%20lu,\"count\":%20lu}\n",
    atomic_load_explicit(&registered, memory_order_relaxed),
    atomic_load_explicit(&count, memory_order_relaxed));
  if (len > 0 && (size_t)len < sizeof json) (void)pwrite(counter_fd, json, (size_t)len, 0);
}

static void ipc_observer(gpointer manager, gpointer value, gpointer unused) {
  (void)manager;
  (void)value; /* Do not inspect, retain, serialize, or forward the IPC payload. */
  (void)unused;
  pthread_mutex_lock(&write_lock);
  atomic_fetch_add_explicit(&count, 1, memory_order_relaxed);
  publish_counts();
  pthread_mutex_unlock(&write_lock);
}

__attribute__((constructor)) static void initialize_counter(void) {
  const char *enabled = getenv("BURRATA_IPC_COUNTER_ENABLE");
  const char *path = getenv("BURRATA_IPC_COUNTER_PATH");
  const char *target_pid = getenv("BURRATA_IPC_COUNTER_PID");
  if (!enabled || strcmp(enabled, "1") != 0 || !path || path[0] != '/' || !target_pid) return;
  char *end = NULL;
  unsigned long expected = strtoul(target_pid, &end, 10);
  if (!end || *end != '\0' || expected == 0 || expected != (unsigned long)getpid()) return;

  counter_fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
  if (counter_fd < 0) return;
  observer_key = g_quark_from_static_string("burrata-private-ipc-counter-observer");
  publish_counts();
}

/* Wry 0.57 registers "ipc" after connecting a generic signal callback. */
gboolean webkit_user_content_manager_register_script_message_handler(gpointer manager, const gchar *name) {
  if (!real_register)
    real_register = (register_fn)dlsym(RTLD_NEXT, "webkit_user_content_manager_register_script_message_handler");
  if (!real_register) return FALSE;

  gboolean ok = real_register(manager, name);
  if (ok && counter_fd >= 0 && g_strcmp0(name, "ipc") == 0 && observer_key &&
      !g_object_get_qdata(G_OBJECT(manager), observer_key)) {
    gulong id = g_signal_connect_data(manager, "script-message-received::ipc",
      G_CALLBACK(ipc_observer), NULL, NULL, 0);
    if (id) {
      g_object_set_qdata(G_OBJECT(manager), observer_key, GUINT_TO_POINTER((guint)id));
      pthread_mutex_lock(&write_lock);
      atomic_fetch_add_explicit(&registered, 1, memory_order_relaxed);
      publish_counts();
      pthread_mutex_unlock(&write_lock);
    }
  }
  return ok;
}
