#include <string.h>
#include <dds/ddsi/ddsi_serdata.h>
#include <dds/ddsi/ddsi_radmin.h>

size_t copy_fragchain_to_buf(const struct ddsi_rdata *fragchain, size_t size, unsigned char *buf)
{
    size_t off = 0;
    for (const struct ddsi_rdata *frag = fragchain; frag != NULL; frag = frag->nextfrag)
    {
        if (frag->maxp1 > (uint32_t)off)
        {
            const unsigned char *payload = DDSI_RMSG_PAYLOADOFF(frag->rmsg, DDSI_RDATA_PAYLOAD_OFF(frag));
            size_t start = (off > frag->min) ? off - frag->min : 0;
            size_t count = frag->maxp1 - (uint32_t)off;
            if (off + count > size) count = size - off;
            memcpy(buf + off, payload + start, count);
            off = frag->maxp1;
            if (off >= size) break;
        }
    }
    return off < size ? off : size;
}
