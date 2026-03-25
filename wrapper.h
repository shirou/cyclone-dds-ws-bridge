// Wrapper header for bindgen -- includes the Cyclone DDS headers we need.

#include <dds/dds.h>
#include <dds/ddsi/ddsi_sertype.h>
#include <dds/ddsi/ddsi_serdata.h>
#include <dds/ddsi/ddsi_radmin.h>

// Declared in helper.c, linked via build.rs
size_t copy_fragchain_to_buf(const struct ddsi_rdata *fragchain, size_t size, unsigned char *buf);
