import struct, sys
# A constructed TIFF/EXIF block: Make, Model, Orientation(1), ExifIFD -> DateTimeOriginal, FNumber.
def ifd(entries, base, next_off=0):
    # entries: (tag, type, count, valuebytes)
    n=len(entries); data=b''; body=struct.pack('<H',n)
    data_off=base+2+12*n+4
    for tag,typ,cnt,val in entries:
        if len(val)<=4:
            body+=struct.pack('<HHI',tag,typ,cnt)+val.ljust(4,b'\0')
        else:
            body+=struct.pack('<HHII',tag,typ,cnt,data_off+len(data)); data+=val
            if len(data)%2: data+=b'\0'
    body+=struct.pack('<I',next_off)
    return body+data
make=b'Apple\0'; model=b'iPhone 15 Pro\0'
hdr=b'II*\0'+struct.pack('<I',8)
# compute IFD0 size first with placeholder exif pointer
ifd0_entries=[(0x010F,2,len(make),make),(0x0110,2,len(model),model),(0x0112,3,1,struct.pack("<H",int(sys.argv[3]) if len(sys.argv)>3 else 1)),(0x8769,4,1,struct.pack('<I',0))]
ifd0=ifd(ifd0_entries,8)
exif_off=8+len(ifd0)
ifd0_entries[3]=(0x8769,4,1,struct.pack('<I',exif_off))
ifd0=ifd(ifd0_entries,8)
dto=b'2026:07:14 18:30:05\0'
exif_entries=[(0x829D,5,1,struct.pack('<II',18,10)),(0x9003,2,len(dto),dto)]
ex=ifd(exif_entries,exif_off)
tiff=hdr+ifd0+ex
app1=b'Exif\0\0'+tiff
jpg=open(sys.argv[1],'rb').read()
out=jpg[:2]+b'\xff\xe1'+struct.pack('>H',len(app1)+2)+app1+jpg[2:]
open(sys.argv[2],'wb').write(out)
